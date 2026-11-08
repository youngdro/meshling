//! GPU device ownership and health.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Why the GPU can't be trusted any more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The device was lost (driver reset, OS killed a submission, `Device::destroy`).
    Lost(String),
    /// An uncaptured wgpu error (validation, out of memory, internal).
    Error(String),
}

/// Records device loss and uncaptured errors so callers stop issuing GPU work and fall back to
/// the CPU renderer instead of panicking (wgpu's default error handler panics).
#[derive(Clone, Debug, Default)]
pub struct DeviceHealth {
    faulted: Arc<AtomicBool>,
    fault: Arc<Mutex<Option<Fault>>>,
}

impl DeviceHealth {
    /// Install the error and device-lost handlers on `device`.
    pub fn watch(device: &wgpu::Device) -> Self {
        let health = Self::default();
        let h = health.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| h.record(Fault::Error(e.to_string()))));
        let h = health.clone();
        device.set_device_lost_callback(move |reason, message| h.record(Fault::Lost(format!("{reason:?}: {message}"))));
        health
    }

    /// The first fault seen, if any.
    pub fn fault(&self) -> Option<Fault> {
        if !self.faulted.load(Ordering::Acquire) {
            return None;
        }
        self.fault.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn is_healthy(&self) -> bool {
        !self.faulted.load(Ordering::Acquire)
    }

    fn record(&self, fault: Fault) {
        let mut slot = self.fault.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(fault);
        }
        self.faulted.store(true, Ordering::Release);
    }
}

/// A device and queue to render with, from a window toolkit (egui) or created headless.
#[derive(Clone, Debug)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub health: DeviceHealth,
    /// Adapter name and backend, for diagnostics.
    pub adapter: String,
}

impl GpuContext {
    /// Wrap a device someone else created (e.g. `egui_wgpu`'s), installing health monitoring.
    pub fn from_parts(device: wgpu::Device, queue: wgpu::Queue, adapter: impl Into<String>) -> Self {
        let health = DeviceHealth::watch(&device);
        Self { device, queue, health, adapter: adapter.into() }
    }

    /// Create a windowless device for offscreen rendering (CLI, MCP, tests, cloud batch).
    /// Honours `WGPU_BACKEND` and `WGPU_ADAPTER_NAME`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Result<Self, ContextError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(&instance, None))
            .map_err(|e| ContextError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("meshling"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| ContextError::RequestDevice(e.to_string()))?;
        Ok(Self::from_parts(device, queue, format!("{} ({:?})", info.name, info.backend)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(String),
    #[error("could not create GPU device: {0}")]
    RequestDevice(String),
}
