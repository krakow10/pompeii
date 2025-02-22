use std::{
    collections::HashSet,
    ffi::{c_void, CStr},
    num::NonZeroU32,
};

use smallvec::SmallVec;
use strum::EnumCount as _;

pub mod fxaa_pass;

/// Store the SPIR-V representation of the shaders in the binary.
pub mod shaders {
    /// The default entry point for shaders is the `main` function.
    pub const ENTRY_POINT_MAIN: &std::ffi::CStr = c"main";

    /// Shader for texture-mapping the entire screen. Useful for post-processing and fullscreen effects.
    pub const FULLSCREEN_VERTEX: &[u32] =
        inline_spirv::include_spirv!("src/shaders/fullscreen_vert.glsl", vert, glsl);
}

/// Target Vulkan API version 1.3 for compatibility with the latest Vulkan features and **reduced fragmentation of extension support**.
const VULKAN_API_VERSION: u32 = ash::vk::API_VERSION_1_3;

/// Set a sane value for the maximum expected number of queue families.
/// A heap allocation is required if the number of queue families exceeds this value.
pub const EXPECTED_MAX_QUEUE_FAMILIES: usize = 8;

/// Set a sane value for the maximum expected number of instance extensions.
/// A heap allocation is required if the number of instance extensions exceeds this value.
pub const EXPECTED_MAX_ENABLED_INSTANCE_EXTENSIONS: usize = 8;

/// Set a sane value for the maximum expected number of physical devices.
/// A heap allocation is required if the number of physical devices exceeds this value.
pub const EXPECTED_MAX_VULKAN_PHYSICAL_DEVICES: usize = 4;

/// Sane maximum number of frames-in-flight before certain heap allocations are required.
pub const EXPECTED_MAX_FRAMES_IN_FLIGHT: usize = 4;

/// The number of nanoseconds in five seconds. Used for sane timeouts on synchronization objects.
pub const FIVE_SECONDS_IN_NANOSECONDS: u64 = 5_000_000_000;

/// Check if a list of extensions contains a specific extension name.
pub fn extensions_list_contains(list: &[ash::vk::ExtensionProperties], ext: &CStr) -> bool {
    list.iter().any(|p| {
        p.extension_name_as_c_str()
            .expect("Extension name received from Vulkan is not a valid C-string")
            == ext
    })
}

/// A minimal helper for converting a Rust reference `&T` to a byte slice over the same memory.
pub fn data_byte_slice<T>(data: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(std::ptr::from_ref(data).cast(), std::mem::size_of::<T>()) }
}

/// The possible errors that may occur when creating a `VulkanCore`.
#[derive(Debug)]
pub enum VulkanCoreError {
    Loading(ash::LoadingError),
    MissingExtension(String),
    MissingLayer(String),
}

/// The main Vulkan library interface. Contains the entry to the Vulkan library and an instance for this app.
pub struct VulkanCore {
    pub version: u32,
    pub api: ash::Entry,
    pub instance: ash::Instance,
    pub khr: Option<ash::khr::surface::Instance>,
    enabled_instance_extensions: HashSet<&'static CStr>,
}

impl VulkanCore {
    /// Create a new `VulkanCore` with the specified instance extensions.
    pub fn new(
        required_extensions: &[&'static CStr],
        optional_extensions: &[&'static CStr],
    ) -> Result<Self, VulkanCoreError> {
        // Attempt to dynamically load the Vulkan API from platform-specific shared libraries.
        let vulkan_api = unsafe { ash::Entry::load().map_err(VulkanCoreError::Loading)? };

        // Determine which extensions are available at runtime.
        let available_extensions = unsafe {
            vulkan_api
                .enumerate_instance_extension_properties(None)
                .expect("Unable to enumerate available Vulkan extensions")
        };

        #[cfg(debug_assertions)]
        println!("INFO: Available instance extensions: {available_extensions:?}\n");

        // Helper lambda for checking if an extension is available.
        let available_contains = |ext: &CStr| extensions_list_contains(&available_extensions, ext);

        // Check that all of the required extensions are available.
        if let Some(missing) = required_extensions
            .iter()
            .find(|ext| !available_contains(ext))
        {
            return Err(VulkanCoreError::MissingExtension(
                missing.to_string_lossy().into_owned(),
            ));
        }

        // Track which extensions are being enabled and convert those extensions to a list of pointers.
        let mut enabled_instance_extensions = std::collections::HashSet::new();
        let mut extension_name_pointers: SmallVec<
            [*const i8; EXPECTED_MAX_ENABLED_INSTANCE_EXTENSIONS],
        > = required_extensions
            .iter()
            .map(|&e| {
                // Add the required extensions to the enabled set.
                enabled_instance_extensions.insert(e);

                // Map the extension to a pointer representation.
                e.as_ptr()
            })
            .collect();

        // Add all of the optional extensions to our creation set if they are available.
        for feature in optional_extensions {
            if available_contains(feature) {
                #[cfg(debug_assertions)]
                println!("INFO: Enabling optional extension '{feature:?}'");

                enabled_instance_extensions.insert(feature);
                extension_name_pointers.push(feature.as_ptr());
            }
        }

        // Optionally, enable `VK_EXT_swapchain_colorspace` if is is available and the dependent `VK_KHR_surface` is requested.
        let requiring_khr_surface = required_extensions.contains(&ash::khr::surface::NAME);
        if requiring_khr_surface && available_contains(ash::ext::swapchain_colorspace::NAME) {
            #[cfg(debug_assertions)]
            println!("INFO: Enabling VK_EXT_swapchain_colorspace extension");

            enabled_instance_extensions.insert(ash::ext::swapchain_colorspace::NAME);
            extension_name_pointers.push(ash::ext::swapchain_colorspace::NAME.as_ptr());
        }

        // Optionally, enable `VK_KHR_get_surface_capabilities2` and `VK_EXT_surface_maintenance1` if they are available and the dependent `VK_KHR_surface` is requested.
        if requiring_khr_surface
            && available_contains(ash::khr::get_surface_capabilities2::NAME)
            && available_contains(ash::ext::surface_maintenance1::NAME)
        {
            #[cfg(debug_assertions)]
            println!("INFO: Enabling VK_KHR_get_surface_capabilities2 and VK_EXT_surface_maintenance1 extensions");

            enabled_instance_extensions.insert(ash::khr::get_surface_capabilities2::NAME);
            extension_name_pointers.push(ash::khr::get_surface_capabilities2::NAME.as_ptr());

            enabled_instance_extensions.insert(ash::ext::surface_maintenance1::NAME);
            extension_name_pointers.push(ash::ext::surface_maintenance1::NAME.as_ptr());
        }

        // Add the debug utility extension if in debug mode.
        #[cfg(debug_assertions)]
        if available_contains(ash::ext::debug_utils::NAME) {
            println!("INFO: Enabling VK_EXT_debug_utils extension");

            enabled_instance_extensions.insert(ash::ext::debug_utils::NAME);
            extension_name_pointers.push(ash::ext::debug_utils::NAME.as_ptr());
        }

        #[cfg(debug_assertions)]
        if extension_name_pointers.spilled() {
            println!(
                "INFO: Extension name pointers list has spilled over to the heap. Extension count {} greater than inline size {}",
                extension_name_pointers.len(),
                extension_name_pointers.inline_size(),
            );
        }

        // Enable validation layers when using a debug build.
        #[cfg(debug_assertions)]
        let layer_names = {
            // Enable the main validation layer from the Khronos Group when using a debug build.
            const DEBUG_LAYERS: [*const i8; 1] = [c"VK_LAYER_KHRONOS_validation".as_ptr()];

            // Check which layers are available at runtime.
            let available_layers = unsafe {
                vulkan_api
                    .enumerate_instance_layer_properties()
                    .expect("Unable to enumerate available Vulkan layers")
            };
            println!("INFO: Available layers: {available_layers:?}\n");

            // Check that all the desired debug layers are available.
            if let Some(missing) = DEBUG_LAYERS.iter().find_map(|&layer_ptr| {
                let layer_cstr = unsafe { CStr::from_ptr(layer_ptr) };
                let layer_exists = available_layers.iter().any(|a| {
                    a.layer_name_as_c_str()
                        .expect("Available layer name is not a valid C-string")
                        == layer_cstr
                });

                if layer_exists {
                    // This layer is not missing from the available layers.
                    None
                } else {
                    // This layer is missing, return it as a `String` to the `find_map`.
                    Some(layer_cstr.to_string_lossy().into_owned())
                }
            }) {
                return Err(VulkanCoreError::MissingLayer(missing));
            }

            DEBUG_LAYERS
        };
        // Disable validation layers when using a release build.
        #[cfg(not(debug_assertions))]
        let layer_names = [];

        // Create a Vulkan instance with the given extensions and layers.
        let vulkan_instance = {
            let application_info = ash::vk::ApplicationInfo {
                p_application_name: c"Pompeii".as_ptr().cast(),
                application_version: ash::vk::make_api_version(0, 0, 1, 0),
                p_engine_name: c"Pompeii".as_ptr().cast(),
                engine_version: ash::vk::make_api_version(0, 0, 1, 0),
                api_version: VULKAN_API_VERSION,
                ..Default::default()
            };
            let instance_info = {
                ash::vk::InstanceCreateInfo {
                    p_application_info: &application_info,
                    enabled_layer_count: layer_names.len() as u32,
                    pp_enabled_layer_names: layer_names.as_ptr(),
                    enabled_extension_count: extension_name_pointers.len() as u32,
                    pp_enabled_extension_names: extension_name_pointers.as_ptr(),
                    ..Default::default()
                }
            };
            unsafe {
                vulkan_api
                    .create_instance(&instance_info, None)
                    .expect("Unable to create Vulkan instance")
            }
        };

        let khr = if requiring_khr_surface {
            Some(ash::khr::surface::Instance::new(
                &vulkan_api,
                &vulkan_instance,
            ))
        } else {
            None
        };
        Ok(Self {
            version: VULKAN_API_VERSION,
            api: vulkan_api,
            instance: vulkan_instance,
            khr,
            enabled_instance_extensions,
        })
    }

    /// Check if the Vulkan instance was created with a specific extension enabled.
    pub fn enabled_instance_extension(&self, ext: &CStr) -> bool {
        self.enabled_instance_extensions.contains(ext)
    }
}

/// Query a physical device for support of a given set of features.
/// Will only check for features which the caller has set to `true` and will set those features which are not supported to `false`.
/// # Note
/// The `p_next` pointers will be ignored by this function and set to `null` before returning.
pub fn query_physical_feature_support(
    instance: &ash::Instance,
    physical_device: ash::vk::PhysicalDevice,
    requested_features: &mut EnginePhysicalDeviceFeatures,
) -> ash::vk::PhysicalDeviceFeatures {
    // Build the feature chain from the provided features.
    let mut feature_chain = ash::vk::PhysicalDeviceFeatures2::default();

    // Ensure there are no circular references in the feature chain.
    requested_features.clear_pointers();

    feature_chain = feature_chain.push_next(&mut requested_features.acceleration_structure);
    feature_chain = feature_chain.push_next(&mut requested_features.buffer_device_address);
    feature_chain = feature_chain.push_next(&mut requested_features.descriptor_indexing);
    feature_chain = feature_chain.push_next(&mut requested_features.dynamic_rendering);
    feature_chain = feature_chain.push_next(&mut requested_features.synchronization2);
    feature_chain = feature_chain.push_next(&mut requested_features.pageable_device_local_memory);
    feature_chain = feature_chain.push_next(&mut requested_features.ray_query);
    feature_chain = feature_chain.push_next(&mut requested_features.ray_tracing);

    let features = {
        // Query the physical device for the selected features.
        unsafe { instance.get_physical_device_features2(physical_device, &mut feature_chain) };
        feature_chain.features
    };

    // Don't confuse the caller with pointers to features.
    requested_features.clear_pointers();

    // Return the features described in the default structure.
    features
}

/// Query the extended properties of a physical device to determine if it supports ray tracing.
#[allow(dead_code)]
pub fn physical_supports_rtx(
    instance: &ash::Instance,
    physical_device: ash::vk::PhysicalDevice,
) -> Option<ash::vk::PhysicalDeviceRayTracingPipelinePropertiesKHR> {
    // Query the physical device for ray tracing support features.
    let mut features = EnginePhysicalDeviceFeatures {
        acceleration_structure: ash::vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
            .acceleration_structure(true),
        ray_query: ash::vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true),
        ray_tracing: ash::vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default()
            .ray_tracing_pipeline(true),
        ..Default::default()
    };
    let _ = query_physical_feature_support(instance, physical_device, &mut features);

    // If the physical device supports ray tracing, return the ray tracing pipeline properties.
    if features.acceleration_structure() && features.ray_tracing() && features.ray_query() {
        let mut ray_tracing_pipeline =
            ash::vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default();
        let mut properties =
            ash::vk::PhysicalDeviceProperties2::default().push_next(&mut ray_tracing_pipeline);
        unsafe { instance.get_physical_device_properties2(physical_device, &mut properties) };
        Some(ray_tracing_pipeline)
    } else {
        None
    }
}

/// The Vulkan physical device features which this engine may utilize. This struct is used to query and report enabled features.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnginePhysicalDeviceFeatures {
    /// Corresponds to `VkPhysicalDeviceAccelerationStructureFeaturesKHR`.
    pub acceleration_structure: ash::vk::PhysicalDeviceAccelerationStructureFeaturesKHR<'static>,

    /// Corresponds to `VkPhysicalDeviceBufferDeviceAddressFeatures`.
    pub buffer_device_address: ash::vk::PhysicalDeviceBufferDeviceAddressFeatures<'static>,

    /// Corresponds to `VkPhysicalDeviceDescriptorIndexingFeatures`.
    pub descriptor_indexing: ash::vk::PhysicalDeviceDescriptorIndexingFeatures<'static>,

    /// Corresponds to `VkPhysicalDeviceDynamicRenderingFeatures`.
    pub dynamic_rendering: ash::vk::PhysicalDeviceDynamicRenderingFeatures<'static>,

    /// Corresponds to `VkPhysicalDevicePageableDeviceLocalMemoryFeaturesEXT`.
    pub pageable_device_local_memory:
        ash::vk::PhysicalDevicePageableDeviceLocalMemoryFeaturesEXT<'static>,

    /// Corresponds to `VkPhysicalDeviceRayQueryFeaturesKHR`.
    pub ray_query: ash::vk::PhysicalDeviceRayQueryFeaturesKHR<'static>,

    /// Corresponds to `VkPhysicalDeviceRayTracingPipelineFeaturesKHR`.
    pub ray_tracing: ash::vk::PhysicalDeviceRayTracingPipelineFeaturesKHR<'static>,

    /// Corresponds to `VkPhysicalDeviceSynchronization2Features`.
    pub synchronization2: ash::vk::PhysicalDeviceSynchronization2Features<'static>,
}
impl EnginePhysicalDeviceFeatures {
    /// A helper to verify that this feature-set contains every enabled (i.e., `true`) feature in the provided mask.
    /// # Note
    /// This doesn't contain checking for sub-properties of a feature. For example, `descriptor_indexing` is not checked because it is only sub-properties.
    pub fn contains_mask(self, mask: &EnginePhysicalDeviceFeatures) -> bool {
        (!mask.acceleration_structure() || self.acceleration_structure())
            && (!mask.buffer_device_address() || self.buffer_device_address())
            && (!mask.dynamic_rendering() || self.dynamic_rendering())
            && (!mask.pageable_device_local_memory() || self.pageable_device_local_memory())
            && (!mask.ray_query() || self.ray_query())
            && (!mask.ray_tracing() || self.ray_tracing())
            && (!mask.synchronization2() || self.synchronization2())
    }

    // Getters for features with a single state representing their support.
    pub fn acceleration_structure(&self) -> bool {
        self.acceleration_structure.acceleration_structure == ash::vk::TRUE
    }
    pub fn buffer_device_address(&self) -> bool {
        self.buffer_device_address.buffer_device_address == ash::vk::TRUE
    }
    pub fn dynamic_rendering(&self) -> bool {
        self.dynamic_rendering.dynamic_rendering == ash::vk::TRUE
    }
    pub fn pageable_device_local_memory(&self) -> bool {
        self.pageable_device_local_memory
            .pageable_device_local_memory
            == ash::vk::TRUE
    }
    pub fn ray_query(&self) -> bool {
        self.ray_query.ray_query == ash::vk::TRUE
    }
    pub fn ray_tracing(&self) -> bool {
        self.ray_tracing.ray_tracing_pipeline == ash::vk::TRUE
    }
    pub fn synchronization2(&self) -> bool {
        self.synchronization2.synchronization2 == ash::vk::TRUE
    }

    /// Clear all feature pointers to `NULL` to avoid circular references.
    pub fn clear_pointers(&mut self) {
        self.acceleration_structure.p_next = std::ptr::null_mut::<c_void>();
        self.buffer_device_address.p_next = std::ptr::null_mut::<c_void>();
        self.descriptor_indexing.p_next = std::ptr::null_mut::<c_void>();
        self.dynamic_rendering.p_next = std::ptr::null_mut::<c_void>();
        self.pageable_device_local_memory.p_next = std::ptr::null_mut::<c_void>();
        self.ray_query.p_next = std::ptr::null_mut::<c_void>();
        self.ray_tracing.p_next = std::ptr::null_mut::<c_void>();
        self.synchronization2.p_next = std::ptr::null_mut::<c_void>();
    }
}

/// Get all physical devices that support the minimum Vulkan API and the required extensions. Sort them by their likelihood of being the desired device.
pub fn get_sorted_physical_devices(
    instance: &ash::Instance,
    minimum_version: u32,
    required_extensions: &[*const i8],
    required_features: &EnginePhysicalDeviceFeatures,
) -> SmallVec<
    [(
        ash::vk::PhysicalDevice,
        ash::vk::PhysicalDeviceProperties,
        EnginePhysicalDeviceFeatures,
    ); EXPECTED_MAX_VULKAN_PHYSICAL_DEVICES],
> {
    /// A helper for scoring a device's dedication to graphics processing.
    fn score_device_type(device_type: ash::vk::PhysicalDeviceType) -> u8 {
        match device_type {
            ash::vk::PhysicalDeviceType::DISCRETE_GPU => 4,
            ash::vk::PhysicalDeviceType::INTEGRATED_GPU => 3,
            ash::vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
            ash::vk::PhysicalDeviceType::CPU => 1,
            _ => 0,
        }
    }

    // Get all physical devices that support Vulkan 1.3.
    // TODO: Investigate if `enumerate_physical_devices` can be replaced with something I can pass a `SmallVec` ref to.
    let physical_devices = unsafe {
        instance
            .enumerate_physical_devices()
            .expect("Unable to enumerate physical devices")
    };

    let mut physical_devices = physical_devices
        .into_iter()
        .filter_map(|device| {
            // Query basic properties of the physical device.
            let properties = unsafe { instance.get_physical_device_properties(device) };

            // Query for the basic extensions available to this physical device.
            let extensions = unsafe {
                instance
                    .enumerate_device_extension_properties(device)
                    .expect("Unable to enumerate device extensions")
            };

            // Ensure every device has their properties printed in debug mode. Called before filtering starts.
            #[cfg(debug_assertions)]
            println!("INFO: Physical device {device:?}: {properties:?}\nPhysical device available extensions: {extensions:?}\n");

            // Ensure the device supports Vulkan 1.3.
            if properties.api_version < minimum_version {
                #[cfg(debug_assertions)]
                println!("INFO: Physical device {device:?} does not support a sufficiently high Vulkan version");
                return None;
            }

            // Ensure the device supports all required features.
            let mut copy_features = *required_features;
            let _ = query_physical_feature_support(instance, device, &mut copy_features);
            if !copy_features.contains_mask(required_features) {
                #[cfg(debug_assertions)]
                println!("INFO: Physical device {device:?}: does not support all the required features {required_features:?}: Actual features {copy_features:?}");
                return None;
            }

            // Ensure the device supports all required extensions.
            if required_extensions.iter().all(|&req| {
                let req = unsafe { CStr::from_ptr(req) };
                let exists = extensions_list_contains(&extensions, req);

                #[cfg(debug_assertions)]
                if !exists {
                    println!("INFO: Physical device {device:?} does not support required extension '{req:?}'");
                }
                exists
            }) {
                Some((device, properties, copy_features))
            } else {
                None
            }
        })
        .collect::<SmallVec<[(ash::vk::PhysicalDevice, ash::vk::PhysicalDeviceProperties, EnginePhysicalDeviceFeatures); 4]>>();

    #[cfg(debug_assertions)]
    if physical_devices.spilled() {
        println!(
            "INFO: Physical devices list has spilled over to the heap. Device count {} greater than inline size {}",
            physical_devices.len(),
            physical_devices.inline_size(),
        );
    }

    // Sort the physical devices by the device type with preference for GPU's, then descending by graphics dedication.
    physical_devices.sort_by(|(_, a, _), (_, b, _)| {
        // Sorting order is reversed (`b.cmp(a)`) to sort the highest scoring device first.
        let device_type_cmp =
            score_device_type(b.device_type).cmp(&score_device_type(a.device_type));

        // If device types are equivalent, then sort by the maximum push constants size.
        if device_type_cmp == std::cmp::Ordering::Equal {
            b.limits
                .max_push_constants_size
                .cmp(&a.limits.max_push_constants_size)
        } else {
            device_type_cmp
        }
    });

    physical_devices
}

/// Available queue family types and their indices.
/// Also, a map of queue family indices to the number of queues that may be and are currently allocated.
pub struct QueueFamilies {
    pub graphics: Vec<u32>,
    pub compute: Vec<u32>,
    pub transfer: Vec<u32>,
    pub present: Vec<u32>,
    pub queue_families: Vec<ash::vk::QueueFamilyProperties>,
}

#[derive(strum::EnumCount)]
#[repr(usize)]
pub enum QueueType {
    Graphics,
    Compute,
    Present,
    Transfer,
}

/// Get the necessary queue family indices for a logical device capable of graphics, compute, and presentation.
pub fn get_queue_families(
    vulkan: &VulkanCore,
    physical_device: ash::vk::PhysicalDevice,
    surface: ash::vk::SurfaceKHR,
) -> QueueFamilies {
    // Get the list of available queue families for this device.
    let queue_families = unsafe {
        vulkan
            .instance
            .get_physical_device_queue_family_properties(physical_device)
    };

    #[cfg(debug_assertions)]
    println!("INFO: Queue families: {queue_families:?}\n");

    // Find the queue families for each desired queue type.
    // Use an array with indices to allow compile-time guarantees about the number of queue types.
    let mut type_indices = [const { Vec::<u32>::new() }; QueueType::COUNT];
    for (family_index, queue_family) in queue_families.iter().enumerate() {
        // Get as a present queue family.
        if let Some(khr) = vulkan.khr.as_ref() {
            if unsafe {
                khr.get_physical_device_surface_support(
                    physical_device,
                    family_index as u32,
                    surface,
                )
            }
            .expect("Unable to check if 'present' is supported")
            {
                type_indices[QueueType::Present as usize].push(family_index as u32);
            }
        }

        // Get as a graphics queue family.
        if queue_family
            .queue_flags
            .contains(ash::vk::QueueFlags::GRAPHICS)
        {
            type_indices[QueueType::Graphics as usize].push(family_index as u32);
        }

        // Get as a compute queue family.
        if queue_family
            .queue_flags
            .contains(ash::vk::QueueFlags::COMPUTE)
        {
            type_indices[QueueType::Compute as usize].push(family_index as u32);
        }

        // Get as a transfer queue family.
        if queue_family
            .queue_flags
            .contains(ash::vk::QueueFlags::TRANSFER)
        {
            type_indices[QueueType::Transfer as usize].push(family_index as u32);
        }
    }

    let graphics = std::mem::take(&mut type_indices[QueueType::Graphics as usize]);
    let compute = std::mem::take(&mut type_indices[QueueType::Compute as usize]);
    let transfer = std::mem::take(&mut type_indices[QueueType::Transfer as usize]);
    let present = std::mem::take(&mut type_indices[QueueType::Present as usize]);

    QueueFamilies {
        graphics,
        compute,
        transfer,
        present,
        queue_families,
    }
}

/// Create a Vulkan logical device capable of graphics, compute, and presentation queues.
/// Returns the device and the queue family indices that were requested for use.
/// # Safety
/// The behavior is undefined if the physical device does not support all the requested device extensions or features.
pub fn new_device(
    vulkan: &VulkanCore,
    physical_device: ash::vk::PhysicalDevice,
    surface: ash::vk::SurfaceKHR,
    device_extensions: &[*const i8],
    feature_chain: Option<&mut dyn ash::vk::ExtendsDeviceCreateInfo>,
) -> (ash::Device, QueueFamilies) {
    // Get the necessary queue family indices for the logical device.
    let queue_families = get_queue_families(vulkan, physical_device, surface);

    // Aggregate queue family indices and count the number of queues each family may need allocated.
    let mut family_map = SmallVec::<[u32; EXPECTED_MAX_QUEUE_FAMILIES]>::from_elem(
        0,
        queue_families.queue_families.len(),
    );
    let all_family_types = [
        &queue_families.graphics,
        &queue_families.compute,
        &queue_families.present,
    ];
    for family in all_family_types {
        for index in family {
            family_map[*index as usize] += 1;
        }
    }

    // Allocate a vector of queue priorities capable of handling the largest queue count among all families.
    let priorities = vec![
        1.;
        queue_families
            .queue_families
            .iter()
            .fold(0, |acc, f| acc.max(f.queue_count as usize))
    ];

    // Print a message if the queue family map has spilled over to the heap.
    #[cfg(debug_assertions)]
    if family_map.spilled() {
        println!(
            "INFO: Queue family map has spilled over to the heap. Family count {} greater than inline size {}",
            queue_families.queue_families.len(),
            family_map.inline_size(),
        );
    }

    // Describe the queue families that will be used with the new logical device.
    let queue_info = family_map
        .iter()
        .enumerate()
        .filter_map(|(index, &count)| {
            if count == 0 {
                return None;
            }

            // Limit to the number of available queues. Guaranteed to be non-zero.
            let queue_count = count.min(queue_families.queue_families[index].queue_count);
            let priorities = &priorities[..queue_count as usize];

            Some(ash::vk::DeviceQueueCreateInfo {
                queue_family_index: index as u32,
                queue_count,
                p_queue_priorities: priorities.as_ptr(),
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();

    // Create the logical device with the desired queue families and extensions.
    let device = unsafe {
        let mut device_info = ash::vk::DeviceCreateInfo {
            queue_create_info_count: queue_info.len() as u32,
            p_queue_create_infos: queue_info.as_ptr(),
            enabled_extension_count: device_extensions.len() as u32,
            pp_enabled_extension_names: device_extensions.as_ptr(),
            ..Default::default()
        };

        // Optionally, add additional features to the device creation info.
        if let Some(feature_chain) = feature_chain {
            device_info = device_info.push_next(feature_chain);
        }

        vulkan
            .instance
            .create_device(physical_device, &device_info, None)
            .expect("Unable to create logical device")
    };

    (device, queue_families)
}

/// A helper type for understanding the context of a queue in Vulkan.
pub struct IndexedQueue {
    pub queue: ash::vk::Queue,
    pub family_index: u32,
    pub index: u32,
}
impl IndexedQueue {
    /// Get a queue from a logical device by its family and index.
    pub fn get(device: &ash::Device, family_index: u32, index: u32) -> Self {
        let queue = unsafe { device.get_device_queue(family_index, index) };
        Self {
            queue,
            family_index,
            index,
        }
    }
}

/// A helper for creating shader modules on a logical device.
pub fn create_shader_module(device: &ash::Device, code: &[u32]) -> ash::vk::ShaderModule {
    unsafe {
        device.create_shader_module(
            &ash::vk::ShaderModuleCreateInfo {
                code_size: code.len() << 2, // Expects the code size to be in bytes.
                p_code: code.as_ptr(),
                ..Default::default()
            },
            None,
        )
    }
    .expect("Unable to create shader module")
}

/// Check if the image format is for a depth buffer.
pub fn is_depth_format(format: ash::vk::Format) -> bool {
    use ash::vk::Format;
    matches!(
        format,
        Format::D16_UNORM
            | Format::D16_UNORM_S8_UINT
            | Format::D24_UNORM_S8_UINT
            | Format::D32_SFLOAT
            | Format::D32_SFLOAT_S8_UINT
            | Format::X8_D24_UNORM_PACK32
    )
}

/// Check if the image format is for a stencil buffer.
pub fn is_stencil_format(format: ash::vk::Format) -> bool {
    use ash::vk::Format;
    matches!(
        format,
        Format::S8_UINT
            | Format::D16_UNORM_S8_UINT
            | Format::D24_UNORM_S8_UINT
            | Format::D32_SFLOAT_S8_UINT
    )
}

/// Helper for creating a new image allocated by a `gpu_allocator::vulkan::Allocator`.
pub fn create_image(
    device: &ash::Device,
    memory_allocator: &mut gpu_allocator::vulkan::Allocator,
    image_create_info: &ash::vk::ImageCreateInfo,
    image_name: &str,
) -> (ash::vk::Image, gpu_allocator::vulkan::Allocation) {
    let image = unsafe { device.create_image(image_create_info, None) }
        .expect("Unable to create new image handle");

    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let allocation = memory_allocator
        .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
            name: image_name,
            requirements,
            location: gpu_allocator::MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: gpu_allocator::vulkan::AllocationScheme::DedicatedImage(image),
        })
        .expect("Unable to allocate memory for new image");
    unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }
        .expect("Unable to bind memory to new image");

    (image, allocation)
}

/// Create a new image view for an existing Vulkan image with a specified format and MIP level.
pub fn create_image_view(
    device: &ash::Device,
    image: ash::vk::Image,
    format: ash::vk::Format,
    mip_levels: u32,
) -> ash::vk::ImageView {
    // Determine what kind of image view to create based on the format.
    let aspect_mask = if is_depth_format(format) {
        ash::vk::ImageAspectFlags::DEPTH
    } else if is_stencil_format(format) {
        ash::vk::ImageAspectFlags::STENCIL
    } else {
        ash::vk::ImageAspectFlags::COLOR
    };

    // Create the image view with the specified parameters.
    // NOTE: More parameters will be needed when supporting XR and 3D images.
    let view_info = ash::vk::ImageViewCreateInfo {
        image,
        view_type: ash::vk::ImageViewType::TYPE_2D, // 2D image. Use 2D array for multi-view/XR.
        format,
        components: ash::vk::ComponentMapping::default(),
        subresource_range: ash::vk::ImageSubresourceRange {
            aspect_mask,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: ash::vk::REMAINING_ARRAY_LAYERS, // In case of 3D images, `VK_REMAINING_ARRAY_LAYERS` will consider all remaining layers.
        },
        ..Default::default()
    };
    unsafe {
        device
            .create_image_view(&view_info, None)
            .expect("Unable to create image view")
    }
}

/// Preferences for the image and behavior used with a new swapchain.
#[derive(Clone, Copy, Default)]
pub struct SwapchainPreferences {
    pub format: Option<ash::vk::Format>,
    pub color_space: Option<ash::vk::ColorSpaceKHR>,
    pub present_mode: Option<ash::vk::PresentModeKHR>,
    pub color_samples: Option<ash::vk::SampleCountFlags>,

    /// The preferred extent to use if and only if the surface wants the caller to specify an extent to use.
    pub preferred_extent: Option<ash::vk::Extent2D>,
}

/// Synchronization objects for a frame in flight.
pub struct FrameInFlightSync {
    pub image_available: ash::vk::Semaphore,
    pub image_rendered: ash::vk::Semaphore,
    pub present_complete: ash::vk::Fence,
}

/// A multi-sample anti-aliasing configuration, including the sample count and managed image resources.
struct MultiSampleAntiAliasing {
    pub samples: ash::vk::SampleCountFlags,
    pub images: Vec<(ash::vk::Image, gpu_allocator::vulkan::Allocation)>,
    pub image_views: Vec<ash::vk::ImageView>,
}

/// A Vulkan swapchain with synchronization objects for each frame in flight.
pub struct Swapchain {
    swapchain_device: ash::khr::swapchain::Device,
    handle: ash::vk::SwapchainKHR,
    image_views: Vec<ash::vk::ImageView>,
    images: Vec<ash::vk::Image>,
    format: ash::vk::Format,
    color_space: ash::vk::ColorSpaceKHR,
    present_mode: ash::vk::PresentModeKHR,
    extent: ash::vk::Extent2D,
    frame_syncs: SmallVec<[FrameInFlightSync; EXPECTED_MAX_FRAMES_IN_FLIGHT]>,
    current_frame: usize,
    acquired_index: Option<u32>,
    multisample: Option<MultiSampleAntiAliasing>,
    enabled_swapchain_maintenance1: bool,
}

/// Vulkan uses this "special value" to indicate that the application must set a desired extent without a `current_extent` available.
/// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/VkSurfaceCapabilitiesKHR.html>
pub const SPECIAL_SURFACE_EXTENT: ash::vk::Extent2D = ash::vk::Extent2D {
    width: u32::MAX,
    height: u32::MAX,
};

/// The result of acquiring the next image from the swapchain and advancing to the next frame in flight.
pub struct NextSwapchainImage {
    pub image_view: ash::vk::ImageView,
    pub image_index: u32,
    pub suboptimal: bool,
}

impl Swapchain {
    /// Create a new swapchain with the specified parameters and Vulkan instance.
    /// # Panics
    /// * The `VulkanCore` struct must have a `khr` field that is not `None`.
    pub fn new(
        vulkan: &VulkanCore,
        physical_device: ash::vk::PhysicalDevice,
        logical_device: &ash::Device,
        surface: ash::vk::SurfaceKHR,
        memory_allocator: &mut gpu_allocator::vulkan::Allocator,
        preferences: SwapchainPreferences,
        enabled_swapchain_maintenance1: bool,
        old_swapchain: Option<ash::vk::SwapchainKHR>,
    ) -> Self {
        let khr = vulkan
            .khr
            .as_ref()
            .expect("Vulkan instance does not support the KHR surface extension");
        let surface_capabilities = unsafe {
            khr.get_physical_device_surface_capabilities(physical_device, surface)
                .expect("Unable to get surface capabilities")
        };

        #[cfg(debug_assertions)]
        println!("INFO: Surface capabilities: {surface_capabilities:?}\n");

        // Try to choose the preferred present mode, but fall back to the default of FIFO. Uses the most reasonable and valid image count for each present mode.
        let (present_mode, image_count) = Self::choose_present_mode_and_image_count(
            vulkan,
            physical_device,
            surface,
            &surface_capabilities,
            preferences.present_mode,
        );

        // Determine the image format that is supported and compare it to what is preferred.
        let supported_formats = unsafe {
            khr.get_physical_device_surface_formats(physical_device, surface)
                .expect("Unable to get supported surface formats")
        };

        #[cfg(debug_assertions)]
        println!("INFO: Supported surface formats: {supported_formats:?}\n");

        let &ash::vk::SurfaceFormatKHR {
            format: image_format,
            color_space: image_color_space,
        } = {
            let lazy_is_color = |f| !is_depth_format(f) && !is_stencil_format(f);
            let color_space = preferences
                .color_space
                .unwrap_or(ash::vk::ColorSpaceKHR::SRGB_NONLINEAR);
            if let Some(format) = supported_formats.iter().find(|f| {
                preferences
                    .format
                    .map_or_else(|| lazy_is_color(f.format), |fmt| f.format == fmt)
                    && f.color_space == color_space
            }) {
                // Use the preferred format if it is supported.
                format
            } else {
                // Otherwise, use the first supported format that is neither a depth nor a stencil format.
                supported_formats
                    .iter()
                    .find(|f| lazy_is_color(f.format))
                    .expect("Unable to find a suitable image format")
            }
        };

        // Prefer the post-multiplied alpha composite alpha mode if available to allow blending when supported.
        let composite_alpha = if surface_capabilities
            .supported_composite_alpha
            .contains(ash::vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED)
        {
            ash::vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED
        } else {
            ash::vk::CompositeAlphaFlagsKHR::OPAQUE
        };

        // In practice, window managers may set the surface to a zero extent when minimized.
        if surface_capabilities.max_image_extent.width == 0
            || surface_capabilities.max_image_extent.height == 0
        {
            panic!(
                "Surface capabilities have not been initialized or window has launched minimized"
            );
        }

        let preferred_extent = if surface_capabilities.current_extent == SPECIAL_SURFACE_EXTENT {
            // The current extent is the special value indicating that the caller must set a desired extent.
            // Use the preferred extent if it is set, otherwise use the minimum extent. The maximum image size is sometimes larger than the memory available.
            preferences
                .preferred_extent
                .unwrap_or(surface_capabilities.min_image_extent)
        } else {
            surface_capabilities.current_extent
        };

        // Clamp the extent to ensure it is within the bounds of the surface.
        let extent = ash::vk::Extent2D {
            width: preferred_extent.width.clamp(
                surface_capabilities.min_image_extent.width,
                surface_capabilities.max_image_extent.width,
            ),
            height: preferred_extent.height.clamp(
                surface_capabilities.min_image_extent.height,
                surface_capabilities.max_image_extent.height,
            ),
        };

        #[cfg(debug_assertions)]
        println!("INFO: Swapchain extent: {extent:?}\n");

        // Optionally provide additional present modes if the
        let mut present_modes_ext = if enabled_swapchain_maintenance1 {
            Some(ash::vk::SwapchainPresentModesCreateInfoEXT {
                present_mode_count: 1,
                p_present_modes: &present_mode,
                ..Default::default()
            })
        } else {
            None
        };

        // Create the swapchain with the specified parameters.
        let mut swapchain_info = ash::vk::SwapchainCreateInfoKHR {
            surface,
            min_image_count: image_count,
            image_format,
            image_color_space,
            image_extent: extent,
            image_array_layers: 1, // Always 1 unless stereoscopic-3D / XR is used.
            image_usage: ash::vk::ImageUsageFlags::COLOR_ATTACHMENT,
            image_sharing_mode: ash::vk::SharingMode::EXCLUSIVE, // Only one queue family will access the images.
            pre_transform: surface_capabilities.current_transform, // Do not apply additional transformation to the surface.
            composite_alpha,
            present_mode,
            clipped: ash::vk::TRUE, // Allow shaders to avoid updating regions that are obscured (by other windows, etc.)
            old_swapchain: old_swapchain.unwrap_or_default(),
            ..Default::default()
        };
        if let Some(present_modes_ext) = &mut present_modes_ext {
            swapchain_info = swapchain_info.push_next(present_modes_ext);
        }

        // Get swapchain-specific function pointers for this logical device.
        let swapchain_device = ash::khr::swapchain::Device::new(&vulkan.instance, logical_device);

        // Create the swapchain with the specified parameters.
        let swapchain = unsafe {
            swapchain_device
                .create_swapchain(&swapchain_info, None)
                .expect("Unable to create swapchain")
        };

        // Determine the actual number of images in the swapchain and create image views for each.
        let swapchain_images = unsafe {
            swapchain_device
                .get_swapchain_images(swapchain)
                .expect("Unable to get swapchain images")
        };

        // Determine if the caller is trying to use multiple color samples, and if it is supported.
        let multisample = if let Some(multisample_image_create) = query_multisample_support(
            vulkan,
            physical_device,
            preferences
                .color_samples
                .unwrap_or(ash::vk::SampleCountFlags::TYPE_1),
            image_format,
            extent,
            1,
            ash::vk::ImageUsageFlags::TRANSIENT_ATTACHMENT
                | ash::vk::ImageUsageFlags::COLOR_ATTACHMENT, // Ensure the multisampled image is optimized to be transient.
        ) {
            let multisample_images: Vec<_> = swapchain_images
                .iter()
                .map(|_| {
                    create_image(
                        logical_device,
                        memory_allocator,
                        &multisample_image_create,
                        "Multisample Image",
                    )
                })
                .collect();
            let image_views = multisample_images
                .iter()
                .map(|(i, _)| create_image_view(logical_device, *i, image_format, 1))
                .collect();

            Some(MultiSampleAntiAliasing {
                samples: multisample_image_create.samples,
                images: multisample_images,
                image_views,
            })
        } else {
            None
        };

        // Create image views for each image in the swapchain.
        let swapchain_views = swapchain_images
            .iter()
            .map(|&i| create_image_view(logical_device, i, image_format, 1))
            .collect();

        // Create synchronization objects. Semaphores synchronize between different operations on the GPU; fences synchronize operations between the CPU and GPU.
        // We will have a frame in flight for each image in the swapchain, and at least two so that a new command can be recorded while another is read.
        let frames_in_flight = image_count.max(2) as usize;
        let frame_syncs = std::iter::repeat_with(|| {
            let image_available = unsafe {
                logical_device
                    .create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None)
                    .expect("Unable to create image available semaphore")
            };
            let image_rendered = unsafe {
                logical_device
                    .create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None)
                    .expect("Unable to create render finished semaphore")
            };
            let present_complete = unsafe {
                logical_device
                    .create_fence(
                        &ash::vk::FenceCreateInfo {
                            flags: ash::vk::FenceCreateFlags::SIGNALED,
                            ..Default::default()
                        },
                        None,
                    )
                    .expect("Unable to create present complete fence")
            };
            FrameInFlightSync {
                image_available,
                image_rendered,
                present_complete,
            }
        })
        .take(frames_in_flight)
        .collect();

        #[cfg(debug_assertions)]
        println!("INFO: New Swapchain: Present mode: {image_count} * {present_mode:?}: Format {image_format:?} in {image_color_space:?}\n");

        Self {
            swapchain_device,
            handle: swapchain,
            image_views: swapchain_views,
            images: swapchain_images,
            format: image_format,
            color_space: image_color_space,
            present_mode,
            extent,
            frame_syncs,
            current_frame: 0,
            acquired_index: None,
            multisample,
            enabled_swapchain_maintenance1,
        }
    }

    /// Delete the swapchain and its associated resources before dropping ownership.
    /// # Safety
    /// This function **must** only be called when the owned resources are not currently being processed by the GPU.
    pub fn destroy(
        self,
        logical_device: &ash::Device,
        memory_allocator: &mut gpu_allocator::vulkan::Allocator,
    ) {
        // Destroy resources in the reverse order they were created.
        unsafe {
            // Destroy the synchronization objects.
            for sync in self.frame_syncs {
                logical_device.destroy_semaphore(sync.image_available, None);
                logical_device.destroy_semaphore(sync.image_rendered, None);
                logical_device.destroy_fence(sync.present_complete, None);
            }

            // Destroy the image views and images.
            // NOTE: Do not directly destroy the images managed by the swapchain internally (i.e., the presentation images).
            for image_view in self.image_views {
                logical_device.destroy_image_view(image_view, None);
            }
            if let Some(multisample) = self.multisample {
                for image_view in multisample.image_views {
                    logical_device.destroy_image_view(image_view, None);
                }
                for (image, allocation) in multisample.images {
                    logical_device.destroy_image(image, None);
                    memory_allocator
                        .free(allocation)
                        .expect("Unable to free multisample image allocation");
                }
            }
            self.swapchain_device.destroy_swapchain(self.handle, None);
        }
    }

    /// Helper to attempt to usee the preferred present mode, but falls back to the default of `FIFO` which is always supported.
    /// Also, tries to use the most reasonable and valid image count for whichever present mode is determined.
    /// # Panics
    /// * The `utils::VulkanCore` struct must have a `khr` field that is not `None`.
    pub fn choose_present_mode_and_image_count(
        vulkan: &VulkanCore,
        physical_device: ash::vk::PhysicalDevice,
        surface: ash::vk::SurfaceKHR,
        surface_capabilities: &ash::vk::SurfaceCapabilitiesKHR,
        preferred_present_mode: Option<ash::vk::PresentModeKHR>,
    ) -> (ash::vk::PresentModeKHR, u32) {
        // NOTE: `SurfaceCapabilitiesKHR` specifies the minimum and maximum number of images that any present mode on this surface may have.
        // However, each present mode may have a tighter bound on the min and max than this global value.
        // See below where `SurfaceCapabilities2KHR` is used to get the actual min and max image count for the determined present mode.
        let surface_min_images = surface_capabilities.min_image_count;
        let surface_max_images = NonZeroU32::new(surface_capabilities.max_image_count);

        // Determine which present modes are available.
        let supported_present_modes = unsafe {
            vulkan
                .khr
                .as_ref()
                .unwrap()
                .get_physical_device_surface_present_modes(physical_device, surface)
                .expect("Unable to get supported present modes")
        };

        #[cfg(debug_assertions)]
        println!("INFO: Supported present modes: {supported_present_modes:?}\n");

        // Default to what is guaranteed to be available.
        let preferred_present_mode =
            preferred_present_mode.unwrap_or(ash::vk::PresentModeKHR::FIFO);

        // Attempt to use the preferred present mode, or fallback to a default mode.
        // Choose the number of images based on the present mode and the minimum images required.
        let (present_mode, mut image_count) = supported_present_modes
            .iter()
            .find_map(|&mode| {
                // Don't choose anything other than the preferred present mode in this first pass.
                if mode != preferred_present_mode {
                    return None;
                }

                match preferred_present_mode {
                    // Immediate mode should use the minimum number of images supported.
                    // This mode is used when there is not a concern with screen tearing, only resource usage.
                    ash::vk::PresentModeKHR::IMMEDIATE => Some((mode, 1)),

                    // Use `MAILBOX` to reduce latency and avoid tearing. Generally preferred.
                    // `FIFO` and `FIFO_RELAXED` modes require at least two images for proper vertical synchronization.
                    ash::vk::PresentModeKHR::MAILBOX
                    | ash::vk::PresentModeKHR::FIFO
                    | ash::vk::PresentModeKHR::FIFO_RELAXED => Some((mode, 3)),

                    // No other named present modes currently exist so we default to FIFO.
                    _ => None,
                }
            })
            .unwrap_or((ash::vk::PresentModeKHR::FIFO, 3));

        if vulkan.enabled_instance_extension(ash::ext::surface_maintenance1::NAME) {
            let mut present_mode_ext =
                ash::vk::SurfacePresentModeEXT::default().present_mode(present_mode);
            let surface_info = ash::vk::PhysicalDeviceSurfaceInfo2KHR::default()
                .surface(surface)
                .push_next(&mut present_mode_ext);
            let mut surface_capabilities = ash::vk::SurfaceCapabilities2KHR::default();
            unsafe {
                ash::khr::get_surface_capabilities2::Instance::new(&vulkan.api, &vulkan.instance)
                    .get_physical_device_surface_capabilities2(
                        physical_device,
                        &surface_info,
                        &mut surface_capabilities,
                    )
            }
            .expect("Unable to get extended surface capabilities(2)");

            let present_max =
                NonZeroU32::new(surface_capabilities.surface_capabilities.max_image_count);
            image_count = image_count.clamp(
                surface_capabilities.surface_capabilities.min_image_count,
                present_max.map_or(u32::MAX, NonZeroU32::get),
            );
        } else {
            image_count = image_count.clamp(
                surface_min_images,
                surface_max_images.map_or(u32::MAX, NonZeroU32::get),
            );
        }

        (present_mode, image_count)
    }

    /// Recreate the swapchain using the existing one.
    /// This is useful when the window is resized, or the window is moved to a different monitor.
    /// # Notes
    /// * This function will wait for the logical device to finish its operations on the swapchain before recreating it.
    /// * The framebuffers will be destroyed and must be recreated by the caller.
    pub fn recreate_swapchain(
        &mut self,
        vulkan: &VulkanCore,
        physical_device: ash::vk::PhysicalDevice,
        logical_device: &ash::Device,
        surface: ash::vk::SurfaceKHR,
        memory_allocator: &mut gpu_allocator::vulkan::Allocator,
        preferences: SwapchainPreferences,
    ) {
        let mut stack_var_swapchain = Self::new(
            vulkan,
            physical_device,
            logical_device,
            surface,
            memory_allocator,
            preferences,
            self.enabled_swapchain_maintenance1,
            Some(self.handle),
        );

        // Swap the original swapchain (`self`) with the new one.
        std::mem::swap(self, &mut stack_var_swapchain);
        let old_swapchain = stack_var_swapchain; // Variable rename for clarity.

        unsafe {
            if self.enabled_swapchain_maintenance1 {
                // Wait for the old swapchain to complete its presentation fences.
                let presentation_fences: SmallVec<[ash::vk::Fence; EXPECTED_MAX_FRAMES_IN_FLIGHT]> =
                    old_swapchain
                        .frame_syncs
                        .iter()
                        .map(|s| s.present_complete)
                        .collect();
                logical_device
                    .wait_for_fences(
                        presentation_fences.as_slice(),
                        true,
                        FIVE_SECONDS_IN_NANOSECONDS,
                    )
                    .expect("Unable to wait for the logical device to finish its operations");
            } else {
                // Wait for the logical device to finish its operations on the swapchain.
                // This is not particularly optimal.
                logical_device
                    .device_wait_idle()
                    .expect("Unable to wait for the logical device to finish its operations");
            }
        }

        // Destroy the old swapchain and its associated resources.
        old_swapchain.destroy(logical_device, memory_allocator);
    }

    /// Acquire the next image in the swapchain. Maintain the index of the acquired image.
    pub fn acquire_next_image(&mut self) -> ash::prelude::VkResult<NextSwapchainImage> {
        // Acquire the next image in the swapchain, using the same fence to signal completion.
        let (acquired_index, suboptimal) = unsafe {
            self.swapchain_device.acquire_next_image(
                self.handle,
                FIVE_SECONDS_IN_NANOSECONDS,
                self.image_available(),
                ash::vk::Fence::null(),
            )?
        };

        // Update our internal state with the acquired image's index.
        self.acquired_index = Some(acquired_index);

        Ok(NextSwapchainImage {
            image_view: self.image_views[acquired_index as usize],
            image_index: acquired_index,
            suboptimal,
        })
    }

    /// Present the next image in the swapchain. Return whether the swapchain is suboptimal for the surface on success.
    pub fn present(
        &mut self,
        present_queue: ash::vk::Queue,
        use_present_fence: bool,
    ) -> ash::prelude::VkResult<bool> {
        let acquired_index = self
            .acquired_index
            .take()
            .expect("No image has been acquired by the swapchain before presenting");

        // Optionally, use a present fence to signal completion of the presentation operation.
        // This is only present with device extension `VK_EXT_swapchain_maintenance1`.
        let fence_info = if use_present_fence {
            Some(ash::vk::SwapchainPresentFenceInfoEXT {
                swapchain_count: 1,
                p_fences: &self.frame_syncs[self.current_frame].present_complete,
                ..Default::default()
            })
        } else {
            None
        };

        let result = unsafe {
            self.swapchain_device.queue_present(
                present_queue,
                &ash::vk::PresentInfoKHR {
                    p_next: fence_info
                        .as_ref()
                        .map_or(std::ptr::null(), |f| std::ptr::from_ref(f).cast()),
                    wait_semaphore_count: 1,
                    p_wait_semaphores: &self.image_rendered(),
                    swapchain_count: 1,
                    p_swapchains: &self.handle,
                    p_image_indices: &acquired_index, // Needs to have the same number of entries as `swapchain_count`, i.e. 1.
                    ..Default::default()
                },
            )
        };

        // Advance the current frame index to the next after successfully submitting the presentation command.
        self.current_frame = (self.current_frame + 1) % self.frame_syncs.len();

        result
    }

    // Swapchain getters.
    pub fn current_frame(&self) -> usize {
        self.current_frame
    }
    pub fn extent(&self) -> ash::vk::Extent2D {
        self.extent
    }
    pub fn frames_in_flight(&self) -> usize {
        self.frame_syncs.len()
    }
    pub fn frame_syncs(&self) -> &[FrameInFlightSync] {
        &self.frame_syncs
    }
    pub fn image_available(&self) -> ash::vk::Semaphore {
        self.frame_syncs[self.current_frame].image_available
    }
    pub fn image_format(&self) -> ash::vk::Format {
        self.format
    }
    pub fn image_rendered(&self) -> ash::vk::Semaphore {
        self.frame_syncs[self.current_frame].image_rendered
    }
    pub fn image_views(&self) -> &[ash::vk::ImageView] {
        &self.image_views
    }
    pub fn images(&self) -> &[ash::vk::Image] {
        &self.images
    }
    pub fn multisample_count(&self) -> Option<ash::vk::SampleCountFlags> {
        self.multisample.as_ref().map(|m| m.samples)
    }
    pub fn multisample_views(&self) -> Option<&[ash::vk::ImageView]> {
        self.multisample.as_ref().map(|m| m.image_views.as_slice())
    }
    pub fn present_complete(&self) -> ash::vk::Fence {
        self.frame_syncs[self.current_frame].present_complete
    }
    pub fn present_mode(&self) -> ash::vk::PresentModeKHR {
        self.present_mode
    }
}

/// Query the physical device for the supported sample count for color images.
/// Returns `Some(n)` with the `ImageCreateInfo` for the single highest supported multi-sample count (i.e., `n > 1`) if found, else `None`.
pub fn query_multisample_support(
    vulkan: &VulkanCore,
    physical_device: ash::vk::PhysicalDevice,
    samples: ash::vk::SampleCountFlags,
    format: ash::vk::Format,
    extent: ash::vk::Extent2D,
    mip_levels: u32,
    usage: ash::vk::ImageUsageFlags,
) -> Option<ash::vk::ImageCreateInfo> {
    // If only checking for the single-sampling, return early.
    if samples.is_empty() || samples == ash::vk::SampleCountFlags::TYPE_1 {
        return None;
    }

    // Check if the physical device supports the requested sample count.
    let mut image_create = ash::vk::ImageCreateInfo {
        image_type: ash::vk::ImageType::TYPE_2D,
        format,
        extent: ash::vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        },
        mip_levels,
        array_layers: 1,
        samples,
        tiling: ash::vk::ImageTiling::OPTIMAL,
        usage,
        ..Default::default()
    };
    let limits = unsafe {
        vulkan
            .instance
            .get_physical_device_image_format_properties(
                physical_device,
                format,
                image_create.image_type,
                image_create.tiling,
                image_create.usage,
                image_create.flags,
            )
            .expect("Unable to get image format properties")
    };

    // Ensure that the physical device supports at least one color sample per pixel.
    #[cfg(debug_assertions)]
    {
        assert!(
            !limits.sample_counts.is_empty(),
            "Physical device does not support any color sample count"
        );
        println!(
            "INFO: Supported sample counts: {:?}\n",
            limits.sample_counts
        );
    }

    if limits.sample_counts.intersects(samples) {
        // Get the highest requested sample count.
        let mut specific_sample_count =
            ash::vk::SampleCountFlags::from_raw(1 << samples.as_raw().ilog2());

        // Find the highest supported sample count that is also requested.
        while !(specific_sample_count.is_empty()
            || limits.sample_counts.contains(specific_sample_count)
                && samples.contains(specific_sample_count))
        {
            specific_sample_count =
                ash::vk::SampleCountFlags::from_raw(specific_sample_count.as_raw() >> 1);
        }

        #[cfg(debug_assertions)]
        assert!(!specific_sample_count.is_empty(), "Physical device does not support any of the requested color sample count(s) {samples:?} after claiming it did");

        // Update the image create info with the supported sample count.
        image_create.samples = specific_sample_count;
        Some(image_create)
    } else {
        #[cfg(debug_assertions)]
        println!(
            "WARN: Physical device does not support any of the requested color sample count(s) {samples:?}\n"
        );

        None
    }
}

/// A helper to get a queue family index that supports the required queue flags and prefers sharing
/// the queue family with the specified flags, and not sharing with the rest.
pub fn get_queue_family_index(
    required_capabilities: ash::vk::QueueFlags,
    prefer_sharing_capabilities: ash::vk::QueueFlags,
    queue_families: &QueueFamilies,
) -> u32 {
    queue_families
        .queue_families
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            if f.queue_flags.contains(required_capabilities) {
                Some((
                    i as u32,
                    (f.queue_flags & prefer_sharing_capabilities)
                        .as_raw()
                        .count_ones()
                        .rotate_left(1) as i32
                        - (f.queue_flags & !prefer_sharing_capabilities)
                            .as_raw()
                            .count_ones() as i32,
                ))
            } else {
                None
            }
        })
        .max_by_key(|(_, score)| *score)
        .expect("Unable to find a queue family with the required flags")
        .0
}

/// Create a new device-local buffer with the given data using a staging buffer.
pub fn new_device_buffer(
    device: &ash::Device,
    allocator: &mut gpu_allocator::vulkan::Allocator,
    command_pool: ash::vk::CommandPool,
    queue: ash::vk::Queue,
    data: &[u8],
) -> Result<(ash::vk::Buffer, ash::vk::Fence), ash::vk::Result> {
    // Create a staging buffer to copy the data to the device-local buffer.
    let staging_buffer = unsafe {
        device.create_buffer(
            &ash::vk::BufferCreateInfo::default()
                .size(data.len() as u64)
                .usage(ash::vk::BufferUsageFlags::TRANSFER_SRC),
            None,
        )?
    };

    // Allocate memory for the staging buffer.
    // TODO: Consider allowing for reuse of the staging buffer for multiple transfers.
    let staging_requirements = unsafe { device.get_buffer_memory_requirements(staging_buffer) };
    let mut staging_allocation = allocator
        .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
            name: "Staging buffer",
            requirements: staging_requirements,
            location: gpu_allocator::MemoryLocation::CpuToGpu,
            linear: true, // "Buffers are always linear" as per README.
            allocation_scheme: gpu_allocator::vulkan::AllocationScheme::DedicatedBuffer(
                staging_buffer,
            ),
        })
        .expect("Unable to allocate memory for staging buffer");

    // Bind the staging buffer to the allocated memory.
    unsafe {
        device.bind_buffer_memory(
            staging_buffer,
            staging_allocation.memory(),
            staging_allocation.offset(),
        )?;
    }
    staging_allocation
        .mapped_slice_mut()
        .expect("Staging buffer did not allocate a mapping")
        .copy_from_slice(data);

    // Create the device-local buffer.
    let device_buffer = unsafe {
        device.create_buffer(
            &ash::vk::BufferCreateInfo::default()
                .size(data.len() as u64)
                .usage(
                    ash::vk::BufferUsageFlags::TRANSFER_DST
                        | ash::vk::BufferUsageFlags::STORAGE_BUFFER,
                ),
            None,
        )?
    };

    // Allocate memory for the device-local buffer.
    let device_requirements = unsafe { device.get_buffer_memory_requirements(device_buffer) };
    let device_allocation = allocator
        .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
            name: "Device buffer",
            requirements: device_requirements,
            location: gpu_allocator::MemoryLocation::GpuOnly,
            linear: true, // "Buffers are always linear" as per README.
            allocation_scheme: gpu_allocator::vulkan::AllocationScheme::DedicatedBuffer(
                device_buffer,
            ),
        })
        .expect("Unable to allocate memory for device buffer");

    // Bind the device-local buffer to the allocated memory.
    unsafe {
        device.bind_buffer_memory(
            device_buffer,
            device_allocation.memory(),
            device_allocation.offset(),
        )?;
    }

    // Copy the data from the staging buffer to the device-local buffer.
    let command_buffer = unsafe {
        device.allocate_command_buffers(&ash::vk::CommandBufferAllocateInfo {
            command_pool,
            level: ash::vk::CommandBufferLevel::PRIMARY,
            command_buffer_count: 1,
            ..Default::default()
        })?
    }[0];
    unsafe {
        device.cmd_copy_buffer(
            command_buffer,
            staging_buffer,
            device_buffer,
            &[ash::vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: data.len() as u64,
            }],
        );
    }

    todo!()
}
