//! Device acquisition for callers that do not already have one.
//!
//! In the shipping application the renderer is handed the host's existing
//! `wgpu::Device` (a Blitz custom widget receives one, so does a GPUI or Iced
//! integration) and this module is not used. It exists for headless
//! verification and for the bare-`winit` path, where something has to create
//! the device.
//!
//! There is no silent skip here. If no hardware adapter answers, the software
//! adapter is requested explicitly and [`GpuContext::class`] reports which one
//! is in use, so a run on a software rasteriser is a visible fact rather than
//! an unexplained slowdown.

/// Whether the device runs on a real GPU or a CPU rasteriser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdapterClass {
    /// A discrete, integrated, or virtualised GPU.
    Hardware,
    /// A CPU implementation such as Lavapipe, SwiftShader, or WARP.
    Software,
}

/// Why a device could not be created.
#[derive(Clone, Debug)]
pub enum GpuError {
    /// Neither a hardware nor a software adapter answered. There is no GPU and
    /// no CPU rasteriser installed.
    NoAdapter {
        /// What the hardware request reported.
        hardware: String,
        /// What the software request reported.
        software: String,
    },
    /// An adapter was found but refused to create a device with the limits the
    /// renderer needs.
    DeviceRejected {
        /// Adapter name.
        adapter: String,
        /// What the adapter reported.
        reason: String,
    },
}

impl core::fmt::Display for GpuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoAdapter { hardware, software } => write!(
                f,
                "no wgpu adapter available: hardware request failed ({hardware}) and the software \
                 fallback failed ({software}); install a Vulkan, Metal, DX12, or GL driver, or a \
                 CPU rasteriser such as Lavapipe"
            ),
            Self::DeviceRejected { adapter, reason } => write!(
                f,
                "adapter '{adapter}' refused a device with the renderer's limits: {reason}"
            ),
        }
    }
}

impl core::error::Error for GpuError {}

/// An instance, adapter, device, and queue, created together.
#[derive(Debug)]
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    class: AdapterClass,
}

impl GpuContext {
    /// Create a device, preferring real hardware and falling back to a software
    /// adapter.
    ///
    /// # Errors
    ///
    /// [`GpuError::NoAdapter`] when neither request finds an adapter, and
    /// [`GpuError::DeviceRejected`] when the adapter will not grant the
    /// renderer's limits.
    pub fn headless() -> Result<Self, GpuError> {
        Self::create(false)
    }

    /// Create a device on a software adapter, even when a GPU is present.
    /// Used to prove the renderer produces identical pixels off a CPU
    /// rasteriser.
    ///
    /// # Errors
    ///
    /// Same conditions as [`GpuContext::headless`].
    pub fn headless_software() -> Result<Self, GpuError> {
        Self::create(true)
    }

    fn create(force_software: bool) -> Result<Self, GpuError> {
        // `_from_env` honours WGPU_BACKEND and WGPU_ADAPTER_NAME, which is how
        // a caller pins a specific adapter without recompiling.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());

        let mut hardware_error = String::from("not attempted");
        let adapter = if force_software {
            None
        } else {
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })) {
                Ok(adapter) => Some(adapter),
                Err(err) => {
                    hardware_error = err.to_string();
                    None
                }
            }
        };

        let adapter = match adapter {
            Some(adapter) => adapter,
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            }))
            .map_err(|err| GpuError::NoAdapter {
                hardware: hardware_error.clone(),
                software: err.to_string(),
            })?,
        };

        let info = adapter.get_info();
        let class = match info.device_type {
            wgpu::DeviceType::Cpu => AdapterClass::Software,
            _ => AdapterClass::Hardware,
        };

        // Downlevel defaults keep the renderer runnable on GL ES 3 and on CPU
        // rasterisers; `using_resolution` lifts only the texture-size caps to
        // what this adapter actually supports.
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vitrum-grid.device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|err| GpuError::DeviceRejected {
            adapter: info.name.clone(),
            reason: err.to_string(),
        })?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            class,
        })
    }

    /// The wgpu instance.
    #[must_use]
    pub const fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// The chosen adapter.
    #[must_use]
    pub const fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// The device the renderer allocates from.
    #[must_use]
    pub const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue the renderer submits to.
    #[must_use]
    pub const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Whether this device is a real GPU or a CPU rasteriser.
    #[must_use]
    pub const fn class(&self) -> AdapterClass {
        self.class
    }

    /// One line naming the adapter, its backend, and its class. Print this
    /// before any measurement so a number is never reported without saying what
    /// produced it.
    #[must_use]
    pub fn describe(&self) -> String {
        let info = self.adapter.get_info();
        let class = match self.class {
            AdapterClass::Hardware => "hardware",
            AdapterClass::Software => "software",
        };
        format!(
            "{} ({:?}, {:?}, {class})",
            info.name, info.backend, info.device_type
        )
    }
}
