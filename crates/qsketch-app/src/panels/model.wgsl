// Model panel: a lit, textured mesh drawn off screen, then copied into the panel.
// Document textures hold display-encoded color, so lighting converts to linear and back.

struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    uv_scale: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var doc_texture: texture_2d<f32>;
@group(0) @binding(2) var doc_sampler: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * u.model * vec4<f32>(position, 1.0);
    out.normal = (u.model * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv * u.uv_scale;
    return out;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Thin surfaces are visible from both sides, so back faces are lit as if they faced the camera.
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    let tex = textureSample(doc_texture, doc_sampler, in.uv);
    // Premultiplied: transparent pixels show as mid gray.
    let base = pow(tex.rgb + vec3<f32>(0.5) * (1.0 - tex.a), vec3<f32>(2.2));
    let key = max(dot(n, normalize(vec3<f32>(0.45, 0.8, 0.55))), 0.0);
    let sky = 0.5 + 0.5 * n.y;
    let lit = base * (0.35 + 0.3 * sky + 0.65 * key);
    return vec4<f32>(pow(lit, vec3<f32>(1.0 / 2.2)), 1.0);
}

@group(0) @binding(3) var blit_texture: texture_2d<f32>;
@group(0) @binding(4) var blit_sampler: sampler;

struct BlitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> BlitOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - vec2<f32>(1.0);
    var out: BlitOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_blit(in: BlitOut) -> @location(0) vec4<f32> {
    return textureSample(blit_texture, blit_sampler, in.uv);
}
