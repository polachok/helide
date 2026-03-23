struct Uniforms {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    // Triangle strip: 0=TL, 1=TR, 2=BL, 3=BR
    let x = select(0.0, 1.0, (in.vertex_index & 1u) != 0u);
    let y = select(0.0, 1.0, (in.vertex_index & 2u) != 0u);

    let pixel_pos = in.pos + vec2<f32>(x, y) * in.size;

    // Convert pixel coords to clip space: [0, screen] -> [-1, 1]
    let clip = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        -(pixel_pos.y / uniforms.screen_size.y * 2.0 - 1.0),
    );

    var out: VertexOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
