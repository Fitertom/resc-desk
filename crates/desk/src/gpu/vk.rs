//! The part of Vulkan 1.3 the renderer uses, written out by hand (no bindings crate):
//! structures in their C layout, the constants, and the entry points, loaded at run time
//! from libvulkan.so.1 (dlopen), so the binary starts without Vulkan installed.
#![allow(non_snake_case, dead_code, clippy::too_many_arguments)]
use std::ffi::{c_char, c_void};

/// Every handle, dispatchable (a pointer) or not (64 bits): the same size on 64-bit targets.
pub type H = u64;
pub type P = *const c_void;
const NULL: P = std::ptr::null();

// ------------------------------------------------------------------ constants

pub const SUCCESS: i32 = 0;
pub const TIMEOUT: i32 = 2;
pub const API_1_3: u32 = (1 << 22) | (3 << 12);
pub const API_1_2: u32 = (1 << 22) | (2 << 12);

/// VkPhysicalDeviceDynamicRenderingFeatures: core in 1.3, VK_KHR_dynamic_rendering before
/// (the same structure either way).
#[repr(C)]
pub struct DynamicRenderingFeatures {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub dynamic_rendering: u32,
}
pub const ST_DYNAMIC_RENDERING_FEATURES: u32 = 1000044003;

pub mod st {
    pub const APPLICATION_INFO: u32 = 0;
    pub const INSTANCE_CREATE_INFO: u32 = 1;
    pub const DEVICE_QUEUE_CREATE_INFO: u32 = 2;
    pub const DEVICE_CREATE_INFO: u32 = 3;
    pub const SUBMIT_INFO: u32 = 4;
    pub const MEMORY_ALLOCATE_INFO: u32 = 5;
    pub const FENCE_CREATE_INFO: u32 = 8;
    pub const BUFFER_CREATE_INFO: u32 = 12;
    pub const IMAGE_CREATE_INFO: u32 = 14;
    pub const IMAGE_VIEW_CREATE_INFO: u32 = 15;
    pub const SHADER_MODULE_CREATE_INFO: u32 = 16;
    pub const PIPELINE_SHADER_STAGE_CREATE_INFO: u32 = 18;
    pub const PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: u32 = 19;
    pub const PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: u32 = 20;
    pub const PIPELINE_VIEWPORT_STATE_CREATE_INFO: u32 = 22;
    pub const PIPELINE_RASTERIZATION_STATE_CREATE_INFO: u32 = 23;
    pub const PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: u32 = 24;
    pub const PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: u32 = 26;
    pub const PIPELINE_DYNAMIC_STATE_CREATE_INFO: u32 = 27;
    pub const GRAPHICS_PIPELINE_CREATE_INFO: u32 = 28;
    pub const PIPELINE_LAYOUT_CREATE_INFO: u32 = 30;
    pub const SAMPLER_CREATE_INFO: u32 = 31;
    pub const DESCRIPTOR_SET_LAYOUT_CREATE_INFO: u32 = 32;
    pub const DESCRIPTOR_POOL_CREATE_INFO: u32 = 33;
    pub const DESCRIPTOR_SET_ALLOCATE_INFO: u32 = 34;
    pub const WRITE_DESCRIPTOR_SET: u32 = 35;
    pub const COMMAND_POOL_CREATE_INFO: u32 = 39;
    pub const COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
    pub const COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
    pub const IMAGE_MEMORY_BARRIER: u32 = 45;
    pub const MEMORY_BARRIER: u32 = 46;
    pub const PHYSICAL_DEVICE_VULKAN_1_3_FEATURES: u32 = 53;
    pub const PHYSICAL_DEVICE_FEATURES_2: u32 = 1000059000;
    pub const RENDERING_INFO: u32 = 1000044000;
    pub const RENDERING_ATTACHMENT_INFO: u32 = 1000044001;
    pub const PIPELINE_RENDERING_CREATE_INFO: u32 = 1000044002;
}

pub const FORMAT_B8G8R8A8_UNORM: u32 = 44;
pub const FORMAT_R32G32B32A32_SFLOAT: u32 = 109;
pub const FORMAT_R16G16B16A16_SFLOAT: u32 = 97;

pub mod layout {
    pub const UNDEFINED: u32 = 0;
    pub const GENERAL: u32 = 1;
    pub const COLOR_ATTACHMENT: u32 = 2;
    pub const SHADER_READ: u32 = 5;
    pub const TRANSFER_SRC: u32 = 6;
    pub const TRANSFER_DST: u32 = 7;
    pub const PREINITIALIZED: u32 = 8;
}

pub mod stage {
    pub const TOP: u32 = 0x1;
    pub const VERTEX_INPUT: u32 = 0x4;
    pub const FRAGMENT: u32 = 0x80;
    pub const COLOR_OUTPUT: u32 = 0x400;
    pub const TRANSFER: u32 = 0x1000;
    pub const BOTTOM: u32 = 0x2000;
    pub const HOST: u32 = 0x4000;
}

pub mod access {
    pub const NONE: u32 = 0;
    pub const VERTEX_ATTRIBUTE_READ: u32 = 0x4;
    pub const SHADER_READ: u32 = 0x20;
    pub const COLOR_READ: u32 = 0x80;
    pub const COLOR_WRITE: u32 = 0x100;
    pub const TRANSFER_READ: u32 = 0x800;
    pub const TRANSFER_WRITE: u32 = 0x1000;
    pub const HOST_READ: u32 = 0x2000;
    pub const HOST_WRITE: u32 = 0x4000;
}

pub mod usage {
    pub const TRANSFER_SRC: u32 = 0x1;
    pub const TRANSFER_DST: u32 = 0x2;
    pub const SAMPLED: u32 = 0x4;
    pub const COLOR_ATTACHMENT: u32 = 0x10;
    pub const VERTEX_BUFFER: u32 = 0x80;
}

pub mod mem {
    pub const DEVICE_LOCAL: u32 = 0x1;
    pub const HOST_VISIBLE: u32 = 0x2;
    pub const HOST_COHERENT: u32 = 0x4;
    pub const HOST_CACHED: u32 = 0x8;
}

pub const QUEUE_GRAPHICS: u32 = 0x1;
pub const SHADER_VERTEX: u32 = 0x1;
pub const SHADER_FRAGMENT: u32 = 0x10;
pub const DESCRIPTOR_COMBINED_IMAGE_SAMPLER: u32 = 1;
pub const FILTER_NEAREST: u32 = 0;
pub const FILTER_LINEAR: u32 = 1;
pub const ADDRESS_CLAMP_TO_EDGE: u32 = 2;
pub const TOPOLOGY_TRIANGLE_LIST: u32 = 3;
pub const TOPOLOGY_TRIANGLE_STRIP: u32 = 4;
pub const BLEND_ZERO: u32 = 0;
pub const BLEND_ONE: u32 = 1;
pub const BLEND_ONE_MINUS_SRC_ALPHA: u32 = 7;
pub const DYNAMIC_VIEWPORT: u32 = 0;
pub const DYNAMIC_SCISSOR: u32 = 1;
pub const LOAD_OP_LOAD: u32 = 0;
pub const LOAD_OP_CLEAR: u32 = 1;
pub const BLEND_OP_REVERSE_SUBTRACT: u32 = 2;
pub const STORE_OP_STORE: u32 = 0;
pub const IMAGE_TYPE_2D: u32 = 1;
pub const VIEW_TYPE_2D: u32 = 1;
pub const ASPECT_COLOR: u32 = 1;
pub const BIND_GRAPHICS: u32 = 0;
pub const INPUT_RATE_INSTANCE: u32 = 1;
pub const POOL_RESET_COMMAND_BUFFER: u32 = 0x2;
pub const POOL_FREE_DESCRIPTOR_SET: u32 = 0x1;
pub const ONE_TIME_SUBMIT: u32 = 0x1;
pub const QUEUE_FAMILY_IGNORED: u32 = !0;
/// VK_EXT_queue_family_foreign: whoever else uses a dma-buf (the client's GPU work).
pub const QUEUE_FAMILY_FOREIGN: u32 = !0 - 2;

// ------------------------------------------------------------------ dma-buf import
//
// VK_KHR_external_memory_fd, VK_EXT_external_memory_dma_buf, VK_EXT_image_drm_format_modifier,
// VK_EXT_queue_family_foreign; VK_KHR_external_semaphore_fd for the client's fence;
// VK_EXT_physical_device_drm for which /dev/dri node this GPU is.

pub mod sx {
    pub const SEMAPHORE_CREATE_INFO: u32 = 9;
    pub const PHYSICAL_DEVICE_PROPERTIES_2: u32 = 1000059001;
    pub const FORMAT_PROPERTIES_2: u32 = 1000059002;
    pub const IMAGE_FORMAT_PROPERTIES_2: u32 = 1000059003;
    pub const PHYSICAL_DEVICE_IMAGE_FORMAT_INFO_2: u32 = 1000059004;
    pub const PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO: u32 = 1000071000;
    pub const EXTERNAL_IMAGE_FORMAT_PROPERTIES: u32 = 1000071001;
    pub const EXTERNAL_MEMORY_IMAGE_CREATE_INFO: u32 = 1000072001;
    pub const IMPORT_MEMORY_FD_INFO: u32 = 1000074000;
    pub const MEMORY_FD_PROPERTIES: u32 = 1000074001;
    pub const IMPORT_SEMAPHORE_FD_INFO: u32 = 1000079000;
    pub const MEMORY_DEDICATED_ALLOCATE_INFO: u32 = 1000127001;
    pub const DRM_FORMAT_MODIFIER_PROPERTIES_LIST: u32 = 1000158000;
    pub const PHYSICAL_DEVICE_IMAGE_DRM_FORMAT_MODIFIER_INFO: u32 = 1000158002;
    pub const IMAGE_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO: u32 = 1000158004;
    pub const PHYSICAL_DEVICE_DRM_PROPERTIES: u32 = 1000353000;
}

pub const HANDLE_DMA_BUF: u32 = 0x200;
pub const SEMAPHORE_HANDLE_SYNC_FD: u32 = 0x10;
pub const SEMAPHORE_IMPORT_TEMPORARY: u32 = 1;
pub const TILING_DRM_FORMAT_MODIFIER: u32 = 1000158000;
pub const FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub const FORMAT_FEATURE_SAMPLED_IMAGE: u32 = 1;
pub const EXTERNAL_MEMORY_IMPORTABLE: u32 = 4;

#[repr(C)]
pub struct ExtensionProperties {
    pub name: [c_char; 256],
    pub spec_version: u32,
}

#[repr(C)]
pub struct Properties2 {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub props: DeviceProps,
}

#[repr(C)]
pub struct DrmProperties {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub has_primary: u32,
    pub has_render: u32,
    pub primary_major: i64,
    pub primary_minor: i64,
    pub render_major: i64,
    pub render_minor: i64,
}

#[repr(C)]
pub struct FormatProperties2 {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub linear: u32,
    pub optimal: u32,
    pub buffer: u32,
}

#[repr(C)]
pub struct DrmModifierList {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub count: u32,
    pub props: *mut DrmModifierProps,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DrmModifierProps {
    pub modifier: u64,
    pub plane_count: u32,
    pub features: u32,
}

#[repr(C)]
pub struct ImageFormatInfo2 {
    pub s_type: u32,
    pub p_next: P,
    pub format: u32,
    pub image_type: u32,
    pub tiling: u32,
    pub usage: u32,
    pub flags: u32,
}

#[repr(C)]
pub struct ExternalImageFormatInfo {
    pub s_type: u32,
    pub p_next: P,
    pub handle_type: u32,
}

#[repr(C)]
pub struct ImageDrmModifierInfo {
    pub s_type: u32,
    pub p_next: P,
    pub modifier: u64,
    pub sharing: u32,
    pub qf_count: u32,
    pub qfs: *const u32,
}

#[repr(C)]
pub struct ImageFormatProperties2 {
    pub s_type: u32,
    pub p_next: *mut c_void,
    /// VkImageFormatProperties: extent, levels, layers, samples, max size.
    pub props: [u64; 4],
}

#[repr(C)]
pub struct ExternalImageFormatProperties {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub features: u32,
    pub export_from: u32,
    pub compatible: u32,
}

#[repr(C)]
pub struct ExternalMemoryImageCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub handle_types: u32,
}

#[repr(C)]
pub struct SubresourceLayout {
    pub offset: u64,
    pub size: u64,
    pub row_pitch: u64,
    pub array_pitch: u64,
    pub depth_pitch: u64,
}

#[repr(C)]
pub struct DrmModifierExplicitCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub modifier: u64,
    pub plane_count: u32,
    pub layouts: *const SubresourceLayout,
}

#[repr(C)]
pub struct ImportMemoryFdInfo {
    pub s_type: u32,
    pub p_next: P,
    pub handle_type: u32,
    pub fd: i32,
}

#[repr(C)]
pub struct MemoryFdProperties {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub type_bits: u32,
}

#[repr(C)]
pub struct MemoryDedicatedAllocateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub image: H,
    pub buffer: H,
}

#[repr(C)]
pub struct SemaphoreCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
}

#[repr(C)]
pub struct ImportSemaphoreFdInfo {
    pub s_type: u32,
    pub p_next: P,
    pub semaphore: H,
    pub flags: u32,
    pub handle_type: u32,
    pub fd: i32,
}
pub const REMAINING: u32 = !0;
pub const DEVICE_TYPE_INTEGRATED: u32 = 1;
pub const DEVICE_TYPE_DISCRETE: u32 = 2;
pub const DEVICE_TYPE_VIRTUAL: u32 = 3;
pub const DEVICE_TYPE_CPU: u32 = 4;

// ------------------------------------------------------------------ structures

#[repr(C)]
pub struct ApplicationInfo {
    pub s_type: u32,
    pub p_next: P,
    pub app_name: *const c_char,
    pub app_version: u32,
    pub engine_name: *const c_char,
    pub engine_version: u32,
    pub api_version: u32,
}

#[repr(C)]
pub struct InstanceCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub app_info: *const ApplicationInfo,
    pub layer_count: u32,
    pub layers: *const *const c_char,
    pub ext_count: u32,
    pub exts: *const *const c_char,
}

/// VkPhysicalDeviceProperties (824 bytes), read by offset: only a few fields are needed.
#[repr(C, align(8))]
pub struct DeviceProps(pub [u8; 1024]);

impl DeviceProps {
    pub fn api_version(&self) -> u32 {
        u32::from_ne_bytes(self.0[0..4].try_into().unwrap())
    }
    pub fn vendor(&self) -> u32 {
        u32::from_ne_bytes(self.0[8..12].try_into().unwrap())
    }
    pub fn device_type(&self) -> u32 {
        u32::from_ne_bytes(self.0[16..20].try_into().unwrap())
    }
    pub fn name(&self) -> String {
        let n = &self.0[20..276];
        let end = n.iter().position(|b| *b == 0).unwrap_or(n.len());
        String::from_utf8_lossy(&n[..end]).into_owned()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct QueueFamilyProperties {
    pub flags: u32,
    pub count: u32,
    pub timestamp_bits: u32,
    pub granularity: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryType {
    pub flags: u32,
    pub heap: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryHeap {
    pub size: u64,
    pub flags: u32,
}

#[repr(C)]
pub struct MemoryProperties {
    pub type_count: u32,
    pub types: [MemoryType; 32],
    pub heap_count: u32,
    pub heaps: [MemoryHeap; 16],
}

#[repr(C)]
pub struct Features2 {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub features: [u32; 55],
}

/// VkPhysicalDeviceVulkan13Features; `f[12]` is dynamicRendering.
#[repr(C)]
pub struct Vulkan13Features {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub f: [u32; 15],
}
pub const F13_DYNAMIC_RENDERING: usize = 12;

#[repr(C)]
pub struct DeviceQueueCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub family: u32,
    pub count: u32,
    pub priorities: *const f32,
}

#[repr(C)]
pub struct DeviceCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub queue_count: u32,
    pub queues: *const DeviceQueueCreateInfo,
    pub layer_count: u32,
    pub layers: *const *const c_char,
    pub ext_count: u32,
    pub exts: *const *const c_char,
    pub features: P,
}

#[repr(C)]
pub struct CommandPoolCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub family: u32,
}

#[repr(C)]
pub struct CommandBufferAllocateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub pool: H,
    pub level: u32,
    pub count: u32,
}

#[repr(C)]
pub struct CommandBufferBeginInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub inheritance: P,
}

#[repr(C)]
pub struct SubmitInfo {
    pub s_type: u32,
    pub p_next: P,
    pub wait_count: u32,
    pub wait_sems: *const H,
    pub wait_stages: *const u32,
    pub cmd_count: u32,
    pub cmds: *const H,
    pub signal_count: u32,
    pub signal_sems: *const H,
}

#[repr(C)]
pub struct FenceCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
}

#[repr(C)]
pub struct BufferCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub size: u64,
    pub usage: u32,
    pub sharing: u32,
    pub qf_count: u32,
    pub qfs: *const u32,
}

#[repr(C)]
#[derive(Default)]
pub struct MemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub type_bits: u32,
}

#[repr(C)]
pub struct MemoryAllocateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub size: u64,
    pub type_index: u32,
}

#[repr(C)]
pub struct ImageCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub image_type: u32,
    pub format: u32,
    pub extent: [u32; 3],
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: u32,
    pub usage: u32,
    pub sharing: u32,
    pub qf_count: u32,
    pub qfs: *const u32,
    pub initial_layout: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubresourceRange {
    pub aspect: u32,
    pub base_mip: u32,
    pub level_count: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

impl SubresourceRange {
    pub fn color(base_mip: u32, level_count: u32) -> Self {
        Self { aspect: ASPECT_COLOR, base_mip, level_count, base_layer: 0, layer_count: 1 }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubresourceLayers {
    pub aspect: u32,
    pub mip: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

impl SubresourceLayers {
    pub fn color(mip: u32) -> Self {
        Self { aspect: ASPECT_COLOR, mip, base_layer: 0, layer_count: 1 }
    }
}

#[repr(C)]
pub struct ImageViewCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub image: H,
    pub view_type: u32,
    pub format: u32,
    pub components: [u32; 4],
    pub range: SubresourceRange,
}

#[repr(C)]
pub struct SamplerCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub mag: u32,
    pub min: u32,
    pub mipmap_mode: u32,
    pub address: [u32; 3],
    pub lod_bias: f32,
    pub anisotropy_enable: u32,
    pub max_anisotropy: f32,
    pub compare_enable: u32,
    pub compare_op: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border_color: u32,
    pub unnormalized: u32,
}

#[repr(C)]
pub struct ShaderModuleCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub code_size: usize,
    pub code: *const u32,
}

#[repr(C)]
pub struct DescriptorSetLayoutBinding {
    pub binding: u32,
    pub ty: u32,
    pub count: u32,
    pub stages: u32,
    pub immutable_samplers: *const H,
}

#[repr(C)]
pub struct DescriptorSetLayoutCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub count: u32,
    pub bindings: *const DescriptorSetLayoutBinding,
}

#[repr(C)]
pub struct PushConstantRange {
    pub stages: u32,
    pub offset: u32,
    pub size: u32,
}

#[repr(C)]
pub struct PipelineLayoutCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub set_count: u32,
    pub sets: *const H,
    pub pc_count: u32,
    pub pcs: *const PushConstantRange,
}

#[repr(C)]
pub struct DescriptorPoolSize {
    pub ty: u32,
    pub count: u32,
}

#[repr(C)]
pub struct DescriptorPoolCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub max_sets: u32,
    pub size_count: u32,
    pub sizes: *const DescriptorPoolSize,
}

#[repr(C)]
pub struct DescriptorSetAllocateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub pool: H,
    pub count: u32,
    pub layouts: *const H,
}

#[repr(C)]
pub struct DescriptorImageInfo {
    pub sampler: H,
    pub view: H,
    pub layout: u32,
}

#[repr(C)]
pub struct WriteDescriptorSet {
    pub s_type: u32,
    pub p_next: P,
    pub set: H,
    pub binding: u32,
    pub array_element: u32,
    pub count: u32,
    pub ty: u32,
    pub image_info: *const DescriptorImageInfo,
    pub buffer_info: P,
    pub texel_views: *const H,
}

#[repr(C)]
pub struct SpecializationMapEntry {
    pub id: u32,
    pub offset: u32,
    pub size: usize,
}

#[repr(C)]
pub struct SpecializationInfo {
    pub count: u32,
    pub entries: *const SpecializationMapEntry,
    pub data_size: usize,
    pub data: P,
}

#[repr(C)]
pub struct PipelineShaderStageCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub stage: u32,
    pub module: H,
    pub name: *const c_char,
    pub spec: *const SpecializationInfo,
}

#[repr(C)]
pub struct VertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub rate: u32,
}

#[repr(C)]
pub struct VertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: u32,
    pub offset: u32,
}

#[repr(C)]
pub struct PipelineVertexInputStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub binding_count: u32,
    pub bindings: *const VertexInputBindingDescription,
    pub attr_count: u32,
    pub attrs: *const VertexInputAttributeDescription,
}

#[repr(C)]
pub struct PipelineInputAssemblyStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub topology: u32,
    pub restart: u32,
}

#[repr(C)]
pub struct PipelineViewportStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub viewport_count: u32,
    pub viewports: P,
    pub scissor_count: u32,
    pub scissors: P,
}

#[repr(C)]
pub struct PipelineRasterizationStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub depth_clamp: u32,
    pub discard: u32,
    pub polygon_mode: u32,
    pub cull_mode: u32,
    pub front_face: u32,
    pub depth_bias: u32,
    pub bias_constant: f32,
    pub bias_clamp: f32,
    pub bias_slope: f32,
    pub line_width: f32,
}

#[repr(C)]
pub struct PipelineMultisampleStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub samples: u32,
    pub sample_shading: u32,
    pub min_sample_shading: f32,
    pub sample_mask: *const u32,
    pub alpha_to_coverage: u32,
    pub alpha_to_one: u32,
}

#[repr(C)]
pub struct PipelineColorBlendAttachmentState {
    pub enable: u32,
    pub src_color: u32,
    pub dst_color: u32,
    pub color_op: u32,
    pub src_alpha: u32,
    pub dst_alpha: u32,
    pub alpha_op: u32,
    pub write_mask: u32,
}

#[repr(C)]
pub struct PipelineColorBlendStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub logic_op_enable: u32,
    pub logic_op: u32,
    pub count: u32,
    pub attachments: *const PipelineColorBlendAttachmentState,
    pub constants: [f32; 4],
}

#[repr(C)]
pub struct PipelineDynamicStateCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub count: u32,
    pub states: *const u32,
}

#[repr(C)]
pub struct PipelineRenderingCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub view_mask: u32,
    pub color_count: u32,
    pub color_formats: *const u32,
    pub depth_format: u32,
    pub stencil_format: u32,
}

#[repr(C)]
pub struct GraphicsPipelineCreateInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub stage_count: u32,
    pub stages: *const PipelineShaderStageCreateInfo,
    pub vertex_input: *const PipelineVertexInputStateCreateInfo,
    pub input_assembly: *const PipelineInputAssemblyStateCreateInfo,
    pub tessellation: P,
    pub viewport: *const PipelineViewportStateCreateInfo,
    pub rasterization: *const PipelineRasterizationStateCreateInfo,
    pub multisample: *const PipelineMultisampleStateCreateInfo,
    pub depth_stencil: P,
    pub color_blend: *const PipelineColorBlendStateCreateInfo,
    pub dynamic: *const PipelineDynamicStateCreateInfo,
    pub layout: H,
    pub render_pass: H,
    pub subpass: u32,
    pub base: H,
    pub base_index: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect2D {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[repr(C)]
pub struct BufferImageCopy {
    pub buffer_offset: u64,
    pub row_length: u32,
    pub image_height: u32,
    pub sub: SubresourceLayers,
    pub offset: [i32; 3],
    pub extent: [u32; 3],
}

#[repr(C)]
pub struct ImageCopy {
    pub src_sub: SubresourceLayers,
    pub src_offset: [i32; 3],
    pub dst_sub: SubresourceLayers,
    pub dst_offset: [i32; 3],
    pub extent: [u32; 3],
}

#[repr(C)]
pub struct ImageBlit {
    pub src_sub: SubresourceLayers,
    pub src: [[i32; 3]; 2],
    pub dst_sub: SubresourceLayers,
    pub dst: [[i32; 3]; 2],
}

#[repr(C)]
pub struct ImageMemoryBarrier {
    pub s_type: u32,
    pub p_next: P,
    pub src_access: u32,
    pub dst_access: u32,
    pub old_layout: u32,
    pub new_layout: u32,
    pub src_qf: u32,
    pub dst_qf: u32,
    pub image: H,
    pub range: SubresourceRange,
}

impl ImageMemoryBarrier {
    pub fn new(image: H, range: SubresourceRange, src_access: u32, dst_access: u32, old_layout: u32, new_layout: u32) -> Self {
        Self {
            s_type: st::IMAGE_MEMORY_BARRIER,
            p_next: NULL,
            src_access,
            dst_access,
            old_layout,
            new_layout,
            src_qf: QUEUE_FAMILY_IGNORED,
            dst_qf: QUEUE_FAMILY_IGNORED,
            image,
            range,
        }
    }
}

#[repr(C)]
pub struct MemoryBarrier {
    pub s_type: u32,
    pub p_next: P,
    pub src_access: u32,
    pub dst_access: u32,
}

#[repr(C)]
pub struct RenderingAttachmentInfo {
    pub s_type: u32,
    pub p_next: P,
    pub view: H,
    pub layout: u32,
    pub resolve_mode: u32,
    pub resolve_view: H,
    pub resolve_layout: u32,
    pub load_op: u32,
    pub store_op: u32,
    pub clear: [f32; 4],
}

#[repr(C)]
pub struct RenderingInfo {
    pub s_type: u32,
    pub p_next: P,
    pub flags: u32,
    pub area: Rect2D,
    pub layer_count: u32,
    pub view_mask: u32,
    pub color_count: u32,
    pub colors: *const RenderingAttachmentInfo,
    pub depth: P,
    pub stencil: P,
}

// ------------------------------------------------------------------ entry points

macro_rules! fns {
    ($name:ident { $($f:ident: fn($($a:ty),*) $(-> $r:ty)?;)* }) => {
        #[derive(Clone, Copy)]
        pub struct $name {
            $(pub $f: unsafe extern "system" fn($($a),*) $(-> $r)?,)*
        }
        impl $name {
            /// Every entry point through `get` (a name with its NUL): an error names a missing one.
            pub unsafe fn load(get: impl Fn(&[u8]) -> P) -> Result<Self, String> {
                Ok(Self {
                    $($f: {
                        let p = get(concat!(stringify!($f), "\0").as_bytes());
                        if p.is_null() {
                            return Err(format!("no {}", stringify!($f)));
                        }
                        std::mem::transmute::<P, unsafe extern "system" fn($($a),*) $(-> $r)?>(p)
                    },)*
                })
            }
        }
    };
}

fns!(Global {
    vkCreateInstance: fn(*const InstanceCreateInfo, P, *mut H) -> i32;
    vkEnumerateInstanceVersion: fn(*mut u32) -> i32;
});

fns!(Instance {
    vkDestroyInstance: fn(H, P);
    vkEnumeratePhysicalDevices: fn(H, *mut u32, *mut H) -> i32;
    vkGetPhysicalDeviceProperties: fn(H, *mut DeviceProps);
    vkGetPhysicalDeviceQueueFamilyProperties: fn(H, *mut u32, *mut QueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(H, *mut MemoryProperties);
    vkGetPhysicalDeviceFeatures2: fn(H, *mut Features2);
    vkCreateDevice: fn(H, *const DeviceCreateInfo, P, *mut H) -> i32;
    vkGetDeviceProcAddr: fn(H, *const c_char) -> P;
    vkEnumerateDeviceExtensionProperties: fn(H, *const c_char, *mut u32, *mut ExtensionProperties) -> i32;
    vkGetPhysicalDeviceProperties2: fn(H, *mut Properties2);
    vkGetPhysicalDeviceFormatProperties2: fn(H, u32, *mut FormatProperties2);
    vkGetPhysicalDeviceImageFormatProperties2: fn(H, *const ImageFormatInfo2, *mut ImageFormatProperties2) -> i32;
});

fns!(Device {
    vkDestroyDevice: fn(H, P);
    vkGetDeviceQueue: fn(H, u32, u32, *mut H);
    vkDeviceWaitIdle: fn(H) -> i32;
    vkCreateCommandPool: fn(H, *const CommandPoolCreateInfo, P, *mut H) -> i32;
    vkDestroyCommandPool: fn(H, H, P);
    vkAllocateCommandBuffers: fn(H, *const CommandBufferAllocateInfo, *mut H) -> i32;
    vkBeginCommandBuffer: fn(H, *const CommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(H) -> i32;
    vkResetCommandBuffer: fn(H, u32) -> i32;
    vkQueueSubmit: fn(H, u32, *const SubmitInfo, H) -> i32;
    vkCreateFence: fn(H, *const FenceCreateInfo, P, *mut H) -> i32;
    vkDestroyFence: fn(H, H, P);
    vkWaitForFences: fn(H, u32, *const H, u32, u64) -> i32;
    vkResetFences: fn(H, u32, *const H) -> i32;
    vkCreateBuffer: fn(H, *const BufferCreateInfo, P, *mut H) -> i32;
    vkDestroyBuffer: fn(H, H, P);
    vkGetBufferMemoryRequirements: fn(H, H, *mut MemoryRequirements);
    vkBindBufferMemory: fn(H, H, H, u64) -> i32;
    vkAllocateMemory: fn(H, *const MemoryAllocateInfo, P, *mut H) -> i32;
    vkFreeMemory: fn(H, H, P);
    vkMapMemory: fn(H, H, u64, u64, u32, *mut *mut c_void) -> i32;
    vkCreateImage: fn(H, *const ImageCreateInfo, P, *mut H) -> i32;
    vkDestroyImage: fn(H, H, P);
    vkGetImageMemoryRequirements: fn(H, H, *mut MemoryRequirements);
    vkBindImageMemory: fn(H, H, H, u64) -> i32;
    vkCreateImageView: fn(H, *const ImageViewCreateInfo, P, *mut H) -> i32;
    vkDestroyImageView: fn(H, H, P);
    vkCreateSampler: fn(H, *const SamplerCreateInfo, P, *mut H) -> i32;
    vkDestroySampler: fn(H, H, P);
    vkCreateShaderModule: fn(H, *const ShaderModuleCreateInfo, P, *mut H) -> i32;
    vkDestroyShaderModule: fn(H, H, P);
    vkCreateDescriptorSetLayout: fn(H, *const DescriptorSetLayoutCreateInfo, P, *mut H) -> i32;
    vkDestroyDescriptorSetLayout: fn(H, H, P);
    vkCreatePipelineLayout: fn(H, *const PipelineLayoutCreateInfo, P, *mut H) -> i32;
    vkDestroyPipelineLayout: fn(H, H, P);
    vkCreateGraphicsPipelines: fn(H, H, u32, *const GraphicsPipelineCreateInfo, P, *mut H) -> i32;
    vkDestroyPipeline: fn(H, H, P);
    vkCreateDescriptorPool: fn(H, *const DescriptorPoolCreateInfo, P, *mut H) -> i32;
    vkDestroyDescriptorPool: fn(H, H, P);
    vkAllocateDescriptorSets: fn(H, *const DescriptorSetAllocateInfo, *mut H) -> i32;
    vkFreeDescriptorSets: fn(H, H, u32, *const H) -> i32;
    vkUpdateDescriptorSets: fn(H, u32, *const WriteDescriptorSet, u32, P);
    vkCmdPipelineBarrier: fn(H, u32, u32, u32, u32, *const MemoryBarrier, u32, P, u32, *const ImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(H, H, H, u32, u32, *const BufferImageCopy);
    vkCmdCopyImageToBuffer: fn(H, H, u32, H, u32, *const BufferImageCopy);
    vkCmdCopyImage: fn(H, H, u32, H, u32, u32, *const ImageCopy);
    vkCmdBlitImage: fn(H, H, u32, H, u32, u32, *const ImageBlit, u32);
    vkCmdBeginRendering: fn(H, *const RenderingInfo);
    vkCmdEndRendering: fn(H);
    vkCmdBindPipeline: fn(H, u32, H);
    vkCmdBindDescriptorSets: fn(H, u32, H, u32, u32, *const H, u32, *const u32);
    vkCmdBindVertexBuffers: fn(H, u32, u32, *const H, *const u64);
    vkCmdPushConstants: fn(H, H, u32, u32, u32, P);
    vkCmdSetViewport: fn(H, u32, u32, *const Viewport);
    vkCmdSetScissor: fn(H, u32, u32, *const Rect2D);
    vkCmdDraw: fn(H, u32, u32, u32, u32);
    vkCreateSemaphore: fn(H, *const SemaphoreCreateInfo, P, *mut H) -> i32;
    vkDestroySemaphore: fn(H, H, P);
});

// Entry points of the dma-buf extensions (only there when they are enabled).
fns!(DmaFns {
    vkGetMemoryFdPropertiesKHR: fn(H, u32, i32, *mut MemoryFdProperties) -> i32;
});

fns!(SemFns {
    vkImportSemaphoreFdKHR: fn(H, *const ImportSemaphoreFdInfo) -> i32;
});

// ------------------------------------------------------------------ loader

extern "C" {
    fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
const RTLD_NOW: i32 = 2;

type GetInstanceProcAddr = unsafe extern "system" fn(H, *const c_char) -> P;

/// libvulkan.so.1's vkGetInstanceProcAddr (the library stays loaded for the process).
pub fn loader() -> Result<GetInstanceProcAddr, String> {
    unsafe {
        let mut h = dlopen(b"libvulkan.so.1\0".as_ptr() as _, RTLD_NOW);
        if h.is_null() {
            h = dlopen(b"libvulkan.so\0".as_ptr() as _, RTLD_NOW);
        }
        if h.is_null() {
            return Err("no libvulkan.so.1".into());
        }
        let p = dlsym(h, b"vkGetInstanceProcAddr\0".as_ptr() as _);
        if p.is_null() {
            return Err("no vkGetInstanceProcAddr".into());
        }
        Ok(std::mem::transmute::<*mut c_void, GetInstanceProcAddr>(p))
    }
}

pub fn check(r: i32, what: &str) -> Result<(), String> {
    if r == SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: VkResult {r}"))
    }
}
