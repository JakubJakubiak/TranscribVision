//! Proste rozpoznawanie mówców bez modelu: barwa głosu (współczynniki cepstralne)
//! liczona dla krótkich fragmentów wypowiedzi i grupowana algorytmem k-średnich.

use rustfft::{FftPlanner, num_complex::Complex};

use crate::project::Word;

const RATE: f32 = 16_000.0;
const NFFT: usize = 512;
const HOP: usize = 160;
const BANDS: usize = 26;
const COEFS: usize = 13;

fn mel(f: f32) -> f32 {
    2595.0 * (1.0 + f / 700.0).log10()
}

fn filterbank() -> Vec<Vec<(usize, f32)>> {
    let (lo, hi) = (mel(80.0), mel(7600.0));
    let pts: Vec<f32> = (0..BANDS + 2).map(|i| lo + (hi - lo) * i as f32 / (BANDS + 1) as f32).collect();
    let bin = |m: f32| ((700.0 * (10f32.powf(m / 2595.0) - 1.0)) * NFFT as f32 / RATE) as usize;
    (0..BANDS)
        .map(|b| {
            let (l, c, r) = (bin(pts[b]), bin(pts[b + 1]), bin(pts[b + 2]));
            let mut v = Vec::new();
            for k in l..=r {
                let w = if k <= c { (k - l) as f32 / (c - l).max(1) as f32 } else { (r - k) as f32 / (r - c).max(1) as f32 };
                if w > 0.0 {
                    v.push((k, w));
                }
            }
            v
        })
        .collect()
}

/// Średnie MFCC (bez c0) i ich odchylenie dla fragmentu.
fn features(audio: &[f32], a: usize, b: usize, fb: &[Vec<(usize, f32)>], fft: &dyn rustfft::Fft<f32>) -> Option<Vec<f32>> {
    let win: Vec<f32> = (0..NFFT).map(|i| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (NFFT - 1) as f32).cos()).collect();
    let mut frames: Vec<[f32; COEFS]> = Vec::new();
    let mut pos = a;
    let mut buf = vec![Complex::new(0.0, 0.0); NFFT];
    while pos + NFFT <= b.min(audio.len()) {
        let chunk = &audio[pos..pos + NFFT];
        let energy: f32 = chunk.iter().map(|s| s * s).sum::<f32>() / NFFT as f32;
        pos += HOP;
        if energy < 1e-6 {
            continue;
        }
        for i in 0..NFFT {
            buf[i] = Complex::new(chunk[i] * win[i], 0.0);
        }
        fft.process(&mut buf);
        let logs: Vec<f32> = fb.iter().map(|band| (band.iter().map(|&(k, w)| buf[k].norm_sqr() * w).sum::<f32>() + 1e-9).ln()).collect();
        let mut c = [0f32; COEFS];
        for (j, cj) in c.iter_mut().enumerate() {
            let j1 = j + 1;
            *cj = logs.iter().enumerate().map(|(i, l)| l * (std::f32::consts::PI * j1 as f32 * (i as f32 + 0.5) / BANDS as f32).cos()).sum();
        }
        frames.push(c);
    }
    if frames.len() < 5 {
        return None;
    }
    let n = frames.len() as f32;
    let mut out = vec![0f32; COEFS * 2];
    for f in &frames {
        for j in 0..COEFS {
            out[j] += f[j] / n;
        }
    }
    for f in &frames {
        for j in 0..COEFS {
            out[COEFS + j] += (f[j] - out[j]).powi(2) / n;
        }
    }
    for v in &mut out[COEFS..] {
        *v = v.sqrt();
    }
    Some(out)
}

/// Przypisz słowom mówców 0..k. `audio` to mono 16 kHz.
pub fn diarize(words: &mut [Word], audio: &[f32], k: usize) {
    if k <= 1 || words.is_empty() {
        for w in words.iter_mut() {
            w.speaker = 0;
        }
        return;
    }
    // Fragmenty: kolejne słowa bez dłuższej przerwy, maksymalnie ~2.5 s.
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut s = 0;
    for i in 1..=words.len() {
        // Dziel na końcach zdań i dłuższych przerwach; mówca zmienia się zwykle między zdaniami.
        let sentence_end = words[i - 1].text.ends_with(['.', '?', '!', '…']);
        let split = i == words.len() || words[i].start - words[i - 1].end > 0.6 || sentence_end || words[i - 1].end - words[s].start > 6.0;
        if split {
            chunks.push((s, i));
            s = i;
        }
    }
    let fb = filterbank();
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(NFFT);
    let feats: Vec<Option<Vec<f32>>> = chunks
        .iter()
        .map(|&(a, b)| features(audio, (words[a].start * RATE as f64) as usize, (words[b - 1].end * RATE as f64) as usize, &fb, fft.as_ref()))
        .collect();
    let valid: Vec<usize> = (0..chunks.len()).filter(|&i| feats[i].is_some()).collect();
    if valid.len() < k {
        return;
    }
    // Standaryzacja cech.
    let dim = COEFS * 2;
    let pts: Vec<Vec<f32>> = valid.iter().map(|&i| feats[i].clone().unwrap_or_default()).collect();
    let mut mean = vec![0f32; dim];
    let mut sd = vec![0f32; dim];
    for p in &pts {
        for j in 0..dim {
            mean[j] += p[j] / pts.len() as f32;
        }
    }
    for p in &pts {
        for j in 0..dim {
            sd[j] += (p[j] - mean[j]).powi(2) / pts.len() as f32;
        }
    }
    let pts: Vec<Vec<f32>> = pts.iter().map(|p| (0..dim).map(|j| (p[j] - mean[j]) / sd[j].sqrt().max(1e-6)).collect()).collect();
    let dist = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>();

    // k-means++ (deterministycznie: najdalszy punkt).
    let mut cents: Vec<Vec<f32>> = vec![pts[0].clone()];
    while cents.len() < k {
        let far = (0..pts.len())
            .max_by(|&a, &b| {
                let da = cents.iter().map(|c| dist(&pts[a], c)).fold(f32::MAX, f32::min);
                let db = cents.iter().map(|c| dist(&pts[b], c)).fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        cents.push(pts[far].clone());
    }
    let mut assign = vec![0usize; pts.len()];
    for _ in 0..50 {
        let mut changed = false;
        for (i, p) in pts.iter().enumerate() {
            let best = (0..k).min_by(|&a, &b| dist(p, &cents[a]).total_cmp(&dist(p, &cents[b]))).unwrap_or(0);
            if best != assign[i] {
                assign[i] = best;
                changed = true;
            }
        }
        for (c, cent) in cents.iter_mut().enumerate() {
            let members: Vec<&Vec<f32>> = pts.iter().zip(&assign).filter(|(_, a)| **a == c).map(|(p, _)| p).collect();
            if members.is_empty() {
                continue;
            }
            for j in 0..dim {
                cent[j] = members.iter().map(|m| m[j]).sum::<f32>() / members.len() as f32;
            }
        }
        if !changed {
            break;
        }
    }
    // Numeruj mówców w kolejności pierwszego pojawienia się.
    let mut order: Vec<usize> = Vec::new();
    for &a in &assign {
        if !order.contains(&a) {
            order.push(a);
        }
    }
    let mut chunk_spk = vec![usize::MAX; chunks.len()];
    for (vi, &ci) in valid.iter().enumerate() {
        chunk_spk[ci] = order.iter().position(|&o| o == assign[vi]).unwrap_or(0);
    }
    // Fragmenty bez cech dziedziczą po poprzednim.
    let mut last = 0;
    for s in chunk_spk.iter_mut() {
        if *s == usize::MAX {
            *s = last;
        }
        last = *s;
    }
    // Wygładzanie: krótki fragment otoczony tym samym mówcą przejmuje jego etykietę.
    for ci in 1..chunk_spk.len().saturating_sub(1) {
        let (a, b) = chunks[ci];
        let short = words[b - 1].end - words[a].start < 1.0;
        if short && chunk_spk[ci - 1] == chunk_spk[ci + 1] {
            chunk_spk[ci] = chunk_spk[ci - 1];
        }
    }
    for (ci, &(a, b)) in chunks.iter().enumerate() {
        for w in &mut words[a..b] {
            w.speaker = chunk_spk[ci];
        }
    }
}
