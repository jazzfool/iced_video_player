struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Uniforms {
    rect: vec4<f32>,
}

@group(0) @binding(0)
var tex_y: texture_2d<f32>;

@group(0) @binding(1)
var tex_uv: texture_2d<f32>;

@group(0) @binding(2)
var s: sampler;

@group(0) @binding(3)
var<uniform> uniforms: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var quad = array<vec4<f32>, 6>(
        vec4<f32>(uniforms.rect.xy, 0.0, 0.0),
        vec4<f32>(uniforms.rect.zy, 1.0, 0.0),
        vec4<f32>(uniforms.rect.xw, 0.0, 1.0),
        vec4<f32>(uniforms.rect.zy, 1.0, 0.0),
        vec4<f32>(uniforms.rect.zw, 1.0, 1.0),
        vec4<f32>(uniforms.rect.xw, 0.0, 1.0),
    );

    var out: VertexOutput;
    out.uv = quad[in_vertex_index].zw;
    out.position = vec4<f32>(quad[in_vertex_index].xy, 1.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let yuv2r = vec3<f32>(1.164, 0.0, 1.596);
    let yuv2g = vec3<f32>(1.164, -0.391, -0.813);
    let yuv2b = vec3<f32>(1.164, 2.018, 0.0);

    var yuv = vec3<f32>(0.0);
    yuv.x = textureSample(tex_y, s, in.uv).r - 0.0625;
    yuv.y = textureSample(tex_uv, s, in.uv).r - 0.5;
    yuv.z = textureSample(tex_uv, s, in.uv).g - 0.5;

    var rgb = vec3<f32>(0.0);
    rgb.x = dot(yuv, yuv2r);
    rgb.y = dot(yuv, yuv2g);
    rgb.z = dot(yuv, yuv2b);

    let threshold = rgb <= vec3<f32>(0.04045);
    let hi = pow((rgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    let lo = rgb * vec3<f32>(1.0 / 12.92);
    rgb = select(hi, lo, threshold);

    return vec4<f32>(rgb, 1.0);
}
