#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Zeroable, bytemuck::Pod)]
pub struct Vertex {
    pub position: [f32; 3],
    pub tex_uv: [f32; 2],
    pub normal: [f32; 3],
}

pub const VERTICES_CUBE: &[Vertex] = &[
    // Front
    Vertex {
        position: [-0.5, -0.5, 0.5],
        tex_uv: [0.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    },
    Vertex {
        position: [0.5, -0.5, 0.5],
        tex_uv: [1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        tex_uv: [1.0, 1.0],
        normal: [0.0, 0.0, 1.0],
    },
    Vertex {
        position: [-0.5, 0.5, 0.5],
        tex_uv: [0.0, 1.0],
        normal: [0.0, 0.0, 1.0],
    },
    // Back
    Vertex {
        position: [-0.5, -0.5, -0.5],
        tex_uv: [1.0, 0.0],
        normal: [0.0, 0.0, -1.0],
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        tex_uv: [1.0, 1.0],
        normal: [0.0, 0.0, -1.0],
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        tex_uv: [0.0, 1.0],
        normal: [0.0, 0.0, -1.0],
    },
    Vertex {
        position: [0.5, -0.5, -0.5],
        tex_uv: [0.0, 0.0],
        normal: [0.0, 0.0, -1.0],
    },
    // Left
    Vertex {
        position: [-0.5, -0.5, -0.5],
        tex_uv: [0.0, 0.0],
        normal: [-1.0, 0.0, 0.0],
    },
    Vertex {
        position: [-0.5, -0.5, 0.5],
        tex_uv: [1.0, 0.0],
        normal: [-1.0, 0.0, 0.0],
    },
    Vertex {
        position: [-0.5, 0.5, 0.5],
        tex_uv: [1.0, 1.0],
        normal: [-1.0, 0.0, 0.0],
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        tex_uv: [0.0, 1.0],
        normal: [-1.0, 0.0, 0.0],
    },
    // Right
    Vertex {
        position: [0.5, -0.5, 0.5],
        tex_uv: [0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    },
    Vertex {
        position: [0.5, -0.5, -0.5],
        tex_uv: [1.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        tex_uv: [1.0, 1.0],
        normal: [1.0, 0.0, 0.0],
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        tex_uv: [0.0, 1.0],
        normal: [1.0, 0.0, 0.0],
    },
    // Top
    Vertex {
        position: [-0.5, 0.5, 0.5],
        tex_uv: [0.0, 0.0],
        normal: [0.0, 1.0, 0.0],
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        tex_uv: [1.0, 0.0],
        normal: [0.0, 1.0, 0.0],
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        tex_uv: [1.0, 1.0],
        normal: [0.0, 1.0, 0.0],
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        tex_uv: [0.0, 1.0],
        normal: [0.0, 1.0, 0.0],
    },
    // Bottom
    Vertex {
        position: [-0.5, -0.5, -0.5],
        tex_uv: [0.0, 0.0],
        normal: [0.0, -1.0, 0.0],
    },
    Vertex {
        position: [0.5, -0.5, -0.5],
        tex_uv: [1.0, 0.0],
        normal: [0.0, -1.0, 0.0],
    },
    Vertex {
        position: [0.5, -0.5, 0.5],
        tex_uv: [1.0, 1.0],
        normal: [0.0, -1.0, 0.0],
    },
    Vertex {
        position: [-0.5, -0.5, 0.5],
        tex_uv: [0.0, 1.0],
        normal: [0.0, -1.0, 0.0],
    },
];

pub fn create_vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    use std::mem::size_of;
    wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3, // Position
            },
            wgpu::VertexAttribute {
                offset: size_of::<[f32; 3]>() as wgpu::BufferAddress,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2, // UV
            },
            wgpu::VertexAttribute {
                offset: (size_of::<[f32; 3]>() + size_of::<[f32; 2]>()) as wgpu::BufferAddress,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x3, // Normal
            },
        ],
    }
}
