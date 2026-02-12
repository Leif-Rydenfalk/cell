use crate::vertex::Vertex;
use gltf::Gltf;
use std::path::Path;
use wgpu::util::DeviceExt;

pub struct Model {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
}

pub struct Mesh {
    pub name: String,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_elements: u32,
    pub material_index: Option<usize>,
}

pub struct Material {
    pub name: String,
    pub diffuse_texture: crate::img_utils::RgbaImg,
    pub texture: Option<wgpu::Texture>,
    pub texture_view: Option<wgpu::TextureView>,
    pub bind_group: Option<wgpu::BindGroup>,
}

impl Model {
    pub fn load<P: AsRef<Path>>(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        path: P,
    ) -> Option<Self> {
        let path = path.as_ref();
        let gltf = match Gltf::open(path) {
            Ok(gltf) => gltf,
            Err(err) => {
                eprintln!("Failed to load GLTF from {}: {}", path.display(), err);
                return None;
            }
        };

        let mut meshes = Vec::new();
        let mut materials = Vec::new();

        // Process materials
        for material in gltf.materials() {
            let name = material.name().unwrap_or("unnamed material").to_string();

            // Get the base color texture
            let diffuse_texture = if let Some(pbr) =
                material.pbr_metallic_roughness().base_color_texture()
            {
                let texture = pbr.texture();
                let source = texture.source().source();

                match source {
                    gltf::image::Source::Uri { uri, .. } => {
                        let texture_path = path.parent().unwrap().join(uri);
                        match crate::img_utils::RgbaImg::new(texture_path.to_str().unwrap()) {
                            Some(texture) => Some(texture),
                            None => {
                                eprintln!("Failed to load texture from {}, using fallback", uri);
                                crate::img_utils::RgbaImg::new("./assets/images/example-img.png")
                            }
                        }
                    }
                    _ => crate::img_utils::RgbaImg::new("./assets/images/example-img.png"),
                }
            } else {
                crate::img_utils::RgbaImg::new("./assets/images/example-img.png")
            };

            if let Some(texture) = diffuse_texture {
                materials.push(Material {
                    name,
                    diffuse_texture: texture,
                    bind_group: None,
                    texture: None,
                    texture_view: None,
                });
            }
        }

        // Process meshes
        for mesh in gltf.meshes() {
            let name = mesh.name().unwrap_or("unnamed mesh").to_string();

            for primitive in mesh.primitives() {
                let material_index = primitive.material().index();
                let reader = primitive.reader(|_| {
                    let buffer_data = gltf.blob.as_ref().unwrap();
                    Some(&buffer_data[..])
                });

                let positions = if let Some(iter) = reader.read_positions() {
                    iter.collect::<Vec<_>>()
                } else {
                    continue;
                };

                let normals = if let Some(iter) = reader.read_normals() {
                    iter.collect::<Vec<_>>()
                } else {
                    vec![[0.0, 1.0, 0.0]; positions.len()]
                };

                let tex_coords = if let Some(iter) = reader.read_tex_coords(0) {
                    iter.into_f32().collect::<Vec<_>>()
                } else {
                    vec![[0.0, 0.0]; positions.len()]
                };

                let vertices: Vec<Vertex> = positions
                    .into_iter()
                    .zip(tex_coords.into_iter())
                    .zip(normals.into_iter())
                    .map(|((pos, tex), norm)| Vertex {
                        position: pos,
                        tex_uv: tex,
                        normal: norm,
                    })
                    .collect();

                let indices = if let Some(indices) = reader.read_indices() {
                    indices.into_u32().collect::<Vec<_>>()
                } else {
                    (0..vertices.len() as u32).collect()
                };

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{} Vertex Buffer", name)),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{} Index Buffer", name)),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                meshes.push(Mesh {
                    name: name.clone(),
                    vertex_buffer,
                    index_buffer,
                    num_elements: indices.len() as u32,
                    material_index,
                });
            }
        }

        Some(Model { meshes, materials })
    }

    pub fn create_bind_groups(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout) {
        for material in &mut self.materials {
            let texture_size = wgpu::Extent3d {
                width: material.diffuse_texture.width,
                height: material.diffuse_texture.height,
                depth_or_array_layers: 1,
            };

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("{} Texture", material.name)),
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            material.texture = Some(texture);
            material.texture_view = Some(texture_view.clone());

            material.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
                label: Some(&format!("{} Bind Group", material.name)),
            }));
        }
    }

    pub fn upload_textures(&self, queue: &wgpu::Queue) {
        for material in &self.materials {
            if let (Some(texture), Some(_)) = (&material.texture, &material.bind_group) {
                let texture_size = wgpu::Extent3d {
                    width: material.diffuse_texture.width,
                    height: material.diffuse_texture.height,
                    depth_or_array_layers: 1,
                };

                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &material.diffuse_texture.bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * material.diffuse_texture.width),
                        rows_per_image: Some(material.diffuse_texture.height),
                    },
                    texture_size,
                );
            }
        }
    }
}
