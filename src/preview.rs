//! Podgląd wideo w oknie egui przez wgpu: płaszczyzny YUV i warstwa napisów
//! trafiają do tekstur, a shader WGSL (`preview.wgsl`) składa obraz.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use egui_wgpu::wgpu;

use crate::media::Frame;
use crate::render::Overlay;

/// Dane wymieniane między wątkami a GPU.
#[derive(Default)]
pub struct PreviewData {
    pub frame: Option<Arc<Frame>>,
    pub frame_ver: u64,
    /// Warstwa napisów w rozmiarze klatki (RGBA8, premultiplied).
    pub overlay: Vec<u8>,
    pub overlay_size: (usize, usize),
    pub overlay_ver: u64,
}

impl PreviewData {
    pub fn set_overlay(&mut self, w: usize, h: usize, ov: Option<&Overlay>) {
        if self.overlay_size != (w, h) || self.overlay.len() != w * h * 4 {
            self.overlay = vec![0; w * h * 4];
            self.overlay_size = (w, h);
        } else {
            self.overlay.fill(0);
        }
        if let Some(ov) = ov {
            for oy in 0..ov.h {
                let y = ov.y + oy as i32;
                if y < 0 || y as usize >= h {
                    continue;
                }
                for ox in 0..ov.w {
                    let x = ov.x + ox as i32;
                    if x < 0 || x as usize >= w {
                        continue;
                    }
                    let p = ov.px[oy * ov.w + ox];
                    let i = (y as usize * w + x as usize) * 4;
                    for k in 0..4 {
                        self.overlay[i + k] = (p[k] * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        self.overlay_ver += 1;
    }
}

pub type SharedPreview = Arc<Mutex<PreviewData>>;

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    tex: Option<Textures>,
    frame_ver: u64,
    overlay_ver: u64,
}

struct Textures {
    size: (usize, usize),
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    ov: wgpu::Texture,
    bind: wgpu::BindGroup,
}

pub fn init(rs: &egui_wgpu::RenderState) {
    let device = &rs.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("preview.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("preview.wgsl").into()) });
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("preview"),
        entries: &[tex_entry(0), tex_entry(1), tex_entry(2), tex_entry(3), wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("preview"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let fs = if rs.target_format.is_srgb() { "fs_main_srgb" } else { "fs_main" };
    if std::env::var_os("TV_DEBUG").is_some() {
        eprintln!("format docelowy: {:?}", rs.target_format);
    }
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("preview"),
        layout: Some(&pl),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), targets: &[Some(rs.target_format.into())], compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    rs.renderer.write().callback_resources.insert(Gpu { pipeline, layout, sampler, tex: None, frame_ver: u64::MAX, overlay_ver: u64::MAX });
}

fn make_tex(device: &wgpu::Device, w: usize, h: usize, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w.max(1) as u32, height: h.max(1) as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write(queue: &wgpu::Queue, tex: &wgpu::Texture, data: &[u8], w: usize, h: usize, bpp: usize) {
    if data.len() < w * h * bpp {
        return;
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some((w * bpp) as u32), rows_per_image: Some(h as u32) },
        wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
    );
}

pub struct PreviewCallback {
    pub data: SharedPreview,
}

impl egui_wgpu::CallbackTrait for PreviewCallback {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, _sd: &egui_wgpu::ScreenDescriptor, _enc: &mut wgpu::CommandEncoder, res: &mut egui_wgpu::CallbackResources) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = res.get_mut::<Gpu>() else { return Vec::new() };
        let Ok(d) = self.data.lock() else { return Vec::new() };
        let Some(frame) = d.frame.clone() else { return Vec::new() };
        let size = (frame.w, frame.h);
        if gpu.tex.as_ref().is_none_or(|t| t.size != size) {
            let (cw, ch) = (frame.cw(), frame.ch());
            let y = make_tex(device, size.0, size.1, wgpu::TextureFormat::R8Unorm);
            let u = make_tex(device, cw, ch, wgpu::TextureFormat::R8Unorm);
            let v = make_tex(device, cw, ch, wgpu::TextureFormat::R8Unorm);
            let ov = make_tex(device, size.0, size.1, wgpu::TextureFormat::Rgba8Unorm);
            let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
            let (vy, vu, vv, vo) = (view(&y), view(&u), view(&v), view(&ov));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("preview"),
                layout: &gpu.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&vy) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&vu) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&vv) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&vo) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                ],
            });
            gpu.tex = Some(Textures { size, y, u, v, ov, bind });
            gpu.frame_ver = u64::MAX;
            gpu.overlay_ver = u64::MAX;
        }
        let Some(t) = gpu.tex.as_ref() else { return Vec::new() };
        if gpu.frame_ver != d.frame_ver {
            write(queue, &t.y, &frame.y, frame.w, frame.h, 1);
            write(queue, &t.u, &frame.u, frame.cw(), frame.ch(), 1);
            write(queue, &t.v, &frame.v, frame.cw(), frame.ch(), 1);
            gpu.frame_ver = d.frame_ver;
        }
        if gpu.overlay_ver != d.overlay_ver {
            if d.overlay_size == size {
                write(queue, &t.ov, &d.overlay, size.0, size.1, 4);
            } else {
                write(queue, &t.ov, &vec![0u8; size.0 * size.1 * 4], size.0, size.1, 4);
            }
            gpu.overlay_ver = d.overlay_ver;
        }
        Vec::new()
    }

    fn paint(&self, _info: eframe::egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, res: &egui_wgpu::CallbackResources) {
        let Some(gpu) = res.get::<Gpu>() else { return };
        let Some(t) = gpu.tex.as_ref() else { return };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &t.bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// Zapis klatki jako PPM (do testów z wiersza poleceń).
pub fn write_ppm(f: &Frame, path: &Path) -> Result<()> {
    let mut out = format!("P6\n{} {}\n255\n", f.w, f.h).into_bytes();
    let cw = f.cw();
    for y in 0..f.h {
        for x in 0..f.w {
            let yy = (f.y[y * f.w + x] as f32 - 16.0) * 1.164383;
            let u = f.u[(y / 2) * cw + x / 2] as f32 - 128.0;
            let v = f.v[(y / 2) * cw + x / 2] as f32 - 128.0;
            let r = yy + 1.792741 * v;
            let g = yy - 0.213249 * u - 0.532909 * v;
            let b = yy + 2.112402 * u;
            out.extend([r, g, b].map(|c| c.round().clamp(0.0, 255.0) as u8));
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}
