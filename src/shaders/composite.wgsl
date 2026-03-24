// Composites a region texture onto the swapchain.
struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_pos: vec2<f32>,
    @location(3) uv_size: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@group(0) @binding(0) var region_texture: texture_2d<f32>;
@group(0) @binding(1) var region_sampler: sampler;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let x = select(0.0, 1.0, (in.vertex_index & 1u) != 0u);
    let y = select(0.0, 1.0, (in.vertex_index & 2u) != 0u);
    let clip_pos = in.pos + vec2<f32>(x, y) * in.size;
    let tex_coord = in.uv_pos + vec2<f32>(x, y) * in.uv_size;
    var out: VertexOutput;
    out.position = vec4<f32>(clip_pos, 0.0, 1.0);
    out.tex_coord = tex_coord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(region_texture, region_sampler, in.tex_coord);
}
