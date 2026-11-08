// meshling mesh pipeline. Mirrors meshling-render-cpu: premultiplied colour, bilinear
// clamp-to-edge sampling, opacity and mask coverage multiply the sample; blending is
// fixed-function (see meshling-plan's compositing table).

struct Draw {
    canvas: vec2<f32>,
    opacity: f32,
    use_mask: u32,
};

@group(0) @binding(0) var<uniform> draw: Draw;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var mask: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(p.x / draw.canvas.x * 2.0 - 1.0, 1.0 - p.y / draw.canvas.y * 2.0);
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_draw(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv) * draw.opacity;
    var cover = 1.0;
    if draw.use_mask != 0u {
        cover = textureLoad(mask, vec2<i32>(in.pos.xy), 0).r;
    }
    return c * cover;
}

// Mask coverage: the sample's alpha, composited Normal into an R8 target.
@fragment
fn fs_mask(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(tex, samp, in.uv).a;
    return vec4<f32>(a, 0.0, 0.0, a);
}
