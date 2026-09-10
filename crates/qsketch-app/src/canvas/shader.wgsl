// Canvas shader: maps every viewport pixel back into document space, samples
// the premultiplied composite texture and draws it over a screen-space
// checkerboard, with an optional pixel grid at high zoom.

struct Uniforms {
    viewport_min: vec2<f32>,
    viewport_size: vec2<f32>,
    doc_size: vec2<f32>,
    tex_size: vec2<f32>,
    center: vec2<f32>,
    zoom: f32,
    rotation: f32,
    flip: f32,
    checker_size: f32,
    grid: f32,
    ppp: f32,
    checker_a: vec4<f32>,
    checker_b: vec4<f32>,
    outside: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    var out: VOut;
    let p = quad[i];
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let screen = u.viewport_min + in.uv * u.viewport_size;
    let vc = u.viewport_min + u.viewport_size * 0.5;
    var d = screen - vc;
    let c = cos(-u.rotation);
    let s = sin(-u.rotation);
    d = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
    if (u.flip > 0.5) {
        d.x = -d.x;
    }
    let doc = u.center + d / u.zoom;

    // Sample regardless of branch (uniform control flow), then decide.
    let col = textureSampleLevel(tex, samp, doc / u.tex_size, 0.0);

    if (doc.x < 0.0 || doc.y < 0.0 || doc.x >= u.doc_size.x || doc.y >= u.doc_size.y) {
        return u.outside;
    }

    let cell = floor(screen * u.ppp / u.checker_size);
    let parity = fract((cell.x + cell.y) * 0.5);
    var checker = u.checker_a.rgb;
    if (parity > 0.25) {
        checker = u.checker_b.rgb;
    }
    var rgb = col.rgb + checker * (1.0 - col.a);

    if (u.grid > 0.5) {
        let f = fract(doc);
        let px = 1.0 / u.zoom;
        if (f.x < px || f.y < px) {
            rgb = mix(rgb, vec3<f32>(0.5, 0.5, 0.5), 0.55);
        }
    }
    return vec4<f32>(rgb, 1.0);
}
