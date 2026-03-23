struct Uniforms {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var atlas_texture: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec4<f32>,   // u0, v0, u1, v1
    @location(3) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let x = select(0.0, 1.0, (in.vertex_index & 1u) != 0u);
    let y = select(0.0, 1.0, (in.vertex_index & 2u) != 0u);

    let pixel_pos = in.pos + vec2<f32>(x, y) * in.size;

    let clip = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        -(pixel_pos.y / uniforms.screen_size.y * 2.0 - 1.0),
    );

    // Interpolate UV
    let tex_coord = vec2<f32>(
        mix(in.uv.x, in.uv.z, x),
        mix(in.uv.y, in.uv.w, y),
    );

    var out: VertexOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.tex_coord = tex_coord;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_texture, atlas_sampler, in.tex_coord).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
