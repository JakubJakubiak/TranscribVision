// Podgląd: klatka YUV 4:2:0 (BT.709, zakres ograniczony) -> RGB na GPU
// + nałożenie warstwy napisów (RGBA, premultiplied).

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // Prostokąt z dwóch trójkątów (triangle strip), obszar callbacku egui.
    let x = f32(i & 1u);
    let y = f32((i >> 1u) & 1u);
    var o: VsOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@group(0) @binding(0) var tex_y: texture_2d<f32>;
@group(0) @binding(1) var tex_u: texture_2d<f32>;
@group(0) @binding(2) var tex_v: texture_2d<f32>;
@group(0) @binding(3) var tex_ov: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

fn shade(uv: vec2<f32>) -> vec4<f32> {
    let y = (textureSample(tex_y, samp, uv).r - 16.0 / 255.0) * 1.164383;
    let u = textureSample(tex_u, samp, uv).r - 0.5;
    let v = textureSample(tex_v, samp, uv).r - 0.5;
    let rgb = clamp(vec3<f32>(
        y + 1.792741 * v,
        y - 0.213249 * u - 0.532909 * v,
        y + 2.112402 * u,
    ), vec3<f32>(0.0), vec3<f32>(1.0));
    let ov = textureSample(tex_ov, samp, uv);
    return vec4<f32>(ov.rgb + rgb * (1.0 - ov.a), 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in.uv);
}

// Cel w formacie sRGB oczekuje wartości liniowych.
@fragment
fn fs_main_srgb(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in.uv);
    let lin = select(pow((c.rgb + 0.055) / 1.055, vec3<f32>(2.4)), c.rgb / 12.92, c.rgb <= vec3<f32>(0.04045));
    return vec4<f32>(lin, 1.0);
}
