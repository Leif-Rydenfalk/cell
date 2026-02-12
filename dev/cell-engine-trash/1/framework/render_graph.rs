use super::resources::ResourceManager;

pub struct RenderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub resources: &'a ResourceManager,
    pub backbuffer_view: &'a wgpu::TextureView,
    pub depth_view: &'a wgpu::TextureView,
}

impl<'a> RenderContext<'a> {
    pub fn get_view(&self, name: &str) -> &wgpu::TextureView {
        if name == "Backbuffer" {
            self.backbuffer_view
        } else {
            self.resources
                .get_texture_view(name)
                .map(|v| v.as_ref())
                .unwrap_or_else(|| self.resources.get_dummy_view().as_ref())
        }
    }

    pub fn get_buffer(&self, name: &str) -> Option<&wgpu::Buffer> {
        self.resources.get_buffer(name).map(|b| b.as_ref())
    }
}
