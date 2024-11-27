// SPDX-License-Identifier: GPL-2.0

//! Wrappers for the PCI subsystem
//!
//! C header: [`include/linux/pci.h`](srctree/include/linux/pci.h)

use crate::{
    alloc::flags::*,
    bindings, container_of, device,
    device_id::RawDeviceId,
    devres::Devres,
    driver,
    error::{to_result, Result},
    io::Io,
    irq,
    str::CStr,
    types::{ARef, ForeignOwnable},
    ThisModule,
};
use core::ops::Deref;
use core::fmt;
use kernel::prelude::*;

/// An adapter for the registration of PCI drivers.
pub struct Adapter<T: Driver>(T);

impl<T: Driver + 'static> driver::RegistrationOps for Adapter<T> {
    type RegType = bindings::pci_driver;

    fn register(
        pdrv: &mut Self::RegType,
        name: &'static CStr,
        module: &'static ThisModule,
    ) -> Result {
        pdrv.name = name.as_char_ptr();
        pdrv.probe = Some(Self::probe_callback);
        pdrv.remove = Some(Self::remove_callback);
        pdrv.id_table = T::ID_TABLE.as_ptr();
        pdrv.sriov_configure = Some(Self::sriov_configure_callback);

        // SAFETY: `pdrv` is guaranteed to be a valid `RegType`.
        to_result(unsafe {
            bindings::__pci_register_driver(pdrv as _, module.0, name.as_char_ptr())
        })
    }

    fn unregister(pdrv: &mut Self::RegType) {
        // SAFETY: `pdrv` is guaranteed to be a valid `RegType`.
        unsafe { bindings::pci_unregister_driver(pdrv) }
    }
}

impl<T: Driver + 'static> Adapter<T> {
    extern "C" fn probe_callback(
        pdev: *mut bindings::pci_dev,
        id: *const bindings::pci_device_id,
    ) -> core::ffi::c_int {
        // SAFETY: The PCI core only ever calls the probe callback with a valid `pdev`.
        let dev = unsafe { device::Device::from_raw(&mut (*pdev).dev) };
        // SAFETY: `dev` is guaranteed to be embedded in a valid `struct pci_dev` by the call
        // above.
        let mut pdev = unsafe { Device::from_dev(dev) };

        // SAFETY: The PCI core only ever calls the probe callback with a valid `id`.
        let index = unsafe { (*id).driver_data };
        let raw_id = T::ID_TABLE.id(index as _);

        // SAFETY: `DeviceId` is a `#[repr(transparent)` wrapper of `DeviceId::RawType` and does not
        // add additional invariants, so it's safe to transmute to `DeviceId`.
        let id = unsafe {
            core::mem::transmute::<&<DeviceId as RawDeviceId>::RawType, &DeviceId>(raw_id)
        };
        let info = T::ID_TABLE.info(index as _);

        match T::probe(&mut pdev, id, info) {
            Ok(data) => {
                // Let the `struct pci_dev` own a reference of the driver's private data.
                // SAFETY: By the type invariant `pdev.as_raw` returns a valid pointer to a
                // `struct pci_dev`.
                unsafe { bindings::pci_set_drvdata(pdev.as_raw(), data.into_foreign() as _) };
            }
            Err(err) => return Error::to_errno(err),
        }

        0
    }

    extern "C" fn remove_callback(pdev: *mut bindings::pci_dev) {
        // SAFETY: The PCI core only ever calls the remove callback with a valid `pdev`. `ptr`
        // points to a valid reference of the driver's private data; it was set by
        // `Adapter::probe_callback`.
        let _ = unsafe {
            let ptr = bindings::pci_get_drvdata(pdev);

            KBox::<T>::from_foreign(ptr)
        };
    }

    extern "C" fn sriov_configure_callback(pdev: *mut bindings::pci_dev, num_vfs: i32) -> i32 {
       // SAFETY: The PCI core only ever calls the probe callback with a valid `pdev`.
        let dev = unsafe { device::Device::from_raw(&mut (*pdev).dev) };
        // SAFETY: `dev` is guaranteed to be embedded in a valid `struct pci_dev` by the call
        // above.
        let mut pdev = unsafe { Device::from_dev(dev) };

	match T::sriov_configure(&mut pdev, num_vfs) {
	    Ok(val) => {
		val
	    }
	    Err(err) => Error::to_errno(err)
	}
    }
}

/// Declares a kernel module that exposes a single PCI driver.
///
/// # Example
///
///```ignore
/// kernel::module_pci_driver! {
///     type: MyDriver,
///     name: "Module name",
///     author: "Author name",
///     description: "Description",
///     license: "GPL v2",
/// }
///```
#[macro_export]
macro_rules! module_pci_driver {
($($f:tt)*) => {
    $crate::module_driver!(<T>, $crate::pci::Adapter<T>, { $($f)* });
};
}

/// Abstraction for bindings::pci_device_id.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct DeviceId(bindings::pci_device_id);

impl DeviceId {
    const PCI_ANY_ID: u32 = !0;

    /// PCI_DEVICE macro.
    pub const fn new(vendor: u32, device: u32) -> Self {
        Self(bindings::pci_device_id {
            vendor,
            device,
            subvendor: DeviceId::PCI_ANY_ID,
            subdevice: DeviceId::PCI_ANY_ID,
            class: 0,
            class_mask: 0,
            driver_data: 0,
            override_only: 0,
        })
    }

    /// PCI_DEVICE_CLASS macro.
    pub const fn with_class(class: u32, class_mask: u32) -> Self {
        Self(bindings::pci_device_id {
            vendor: DeviceId::PCI_ANY_ID,
            device: DeviceId::PCI_ANY_ID,
            subvendor: DeviceId::PCI_ANY_ID,
            subdevice: DeviceId::PCI_ANY_ID,
            class,
            class_mask,
            driver_data: 0,
            override_only: 0,
        })
    }
}

// Allow drivers R/O access to the fields of `pci_device_id`; should we prefer accessor functions
// to void exposing C structure fields?
impl Deref for DeviceId {
    type Target = bindings::pci_device_id;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// SAFETY:
// * `DeviceId` is a `#[repr(transparent)` wrapper of `pci_device_id` and does not add
//   additional invariants, so it's safe to transmute to `RawType`.
// * `DRIVER_DATA_OFFSET` is the offset to the `driver_data` field.
unsafe impl RawDeviceId for DeviceId {
    type RawType = bindings::pci_device_id;

    const DRIVER_DATA_OFFSET: usize = core::mem::offset_of!(bindings::pci_device_id, driver_data);
}

/// IdTable type for PCI
pub type IdTable<T> = &'static dyn kernel::device_id::IdTable<DeviceId, T>;

/// The PCI driver trait.
///
/// # Example
///
///```
/// # use kernel::{bindings, device_id::IdArray, pci, sync::Arc};
///
/// struct MyDriver;
/// struct MyDeviceData;
///
/// impl pci::Driver for MyDriver {
///     type IdInfo = ();
///
///     const ID_TABLE: pci::IdTable<Self::IdInfo> = &IdArray::new([
///         (pci::DeviceId::new(bindings::PCI_VENDOR_ID_REDHAT, bindings::PCI_ANY_ID as u32), ())
///     ]);
///
///     fn probe(
///         _pdev: &mut pci::Device,
///         _id: &pci::DeviceId,
///         _id_info: &Self::IdInfo,
///     ) -> Result<Pin<KBox<Self>>> {
///         Err(ENODEV)
///     }
/// }
///```
/// Drivers must implement this trait in order to get a PCI driver registered. Please refer to the
/// `Adapter` documentation for an example.
pub trait Driver {
    /// The type holding information about each device id supported by the driver.
    ///
    /// TODO: Use associated_type_defaults once stabilized:
    ///
    /// type IdInfo: 'static = ();
    type IdInfo: 'static;

    /// The table of device ids supported by the driver.
    const ID_TABLE: IdTable<Self::IdInfo>;

    /// PCI driver probe.
    ///
    /// Called when a new platform device is added or discovered.
    /// Implementers should attempt to initialize the device here.
    fn probe(dev: &mut Device, id: &DeviceId, id_info: &Self::IdInfo) -> Result<Pin<KBox<Self>>>;

    /// PCI SRIOV configuration callback
    fn sriov_configure(dev: &mut Device, num_vfs: i32) -> Result<i32>;
}

/// The PCI device representation.
///
/// A PCI device is based on an always reference counted `device:Device` instance. Cloning a PCI
/// device, hence, also increments the base device' reference count.
///
/// # Invariants
///
/// `Device` hold a valid reference of `ARef<device::Device>` whose underlying `struct device` is a
/// member of a `struct pci_dev`.
#[derive(Clone)]
pub struct Device(ARef<device::Device>);

/// A PCI BAR to perform I/O-Operations on.
///
/// # Invariants
///
/// `Bar` always holds an `Io` inststance that holds a valid pointer to the start of the I/O memory
/// mapped PCI bar and its size.
pub struct Bar<const SIZE: usize = 0> {
    pdev: Device,
    io: Io<SIZE>,
    num: i32,
}

impl<const SIZE: usize> Bar<SIZE> {
    fn new(pdev: Device, num: u32, name: &CStr) -> Result<Self> {
        let len = pdev.resource_len(num)?;
        if len == 0 {
            return Err(ENOMEM);
        }

        // Convert to `i32`, since that's what all the C bindings use.
        let num = i32::try_from(num)?;

        // SAFETY:
        // `pdev` is valid by the invariants of `Device`.
        // `num` is checked for validity by a previous call to `Device::resource_len`.
        // `name` is always valid.
        let ret = unsafe { bindings::pci_request_region(pdev.as_raw(), num, name.as_char_ptr()) };
        if ret != 0 {
            return Err(EBUSY);
        }

        // SAFETY:
        // `pdev` is valid by the invariants of `Device`.
        // `num` is checked for validity by a previous call to `Device::resource_len`.
        // `name` is always valid.
        let ioptr: usize = unsafe { bindings::pci_iomap(pdev.as_raw(), num, 0) } as usize;
        if ioptr == 0 {
            // SAFETY:
            // `pdev` valid by the invariants of `Device`.
            // `num` is checked for validity by a previous call to `Device::resource_len`.
            unsafe { bindings::pci_release_region(pdev.as_raw(), num) };
            return Err(ENOMEM);
        }

        // SAFETY: `ioptr` is guaranteed to be the start of a valid I/O mapped memory region of size
        // `len`.
        let io = match unsafe { Io::new(ioptr, len as usize) } {
            Ok(io) => io,
            Err(err) => {
                // SAFETY:
                // `pdev` is valid by the invariants of `Device`.
                // `ioptr` is guaranteed to be the start of a valid I/O mapped memory region.
                // `num` is checked for validity by a previous call to `Device::resource_len`.
                unsafe { Self::do_release(&pdev, ioptr, num) };
                return Err(err);
            }
        };

        Ok(Bar { pdev, io, num })
    }

    /// # Safety
    ///
    /// `ioptr` must be a valid pointer to the memory mapped PCI bar number `num`.
    unsafe fn do_release(pdev: &Device, ioptr: usize, num: i32) {
        // SAFETY:
        // `pdev` is valid by the invariants of `Device`.
        // `ioptr` is valid by the safety requirements.
        // `num` is valid by the safety requirements.
        unsafe {
            bindings::pci_iounmap(pdev.as_raw(), ioptr as _);
            bindings::pci_release_region(pdev.as_raw(), num);
        }
    }

    fn release(&self) {
        // SAFETY: Safe by the invariants of `Device` and `Bar`.
        unsafe { Self::do_release(&self.pdev, self.io.base_addr(), self.num) };
    }
}

impl Bar {
    fn index_is_valid(index: u32) -> bool {
        // A `struct pci_dev` owns an array of resources with at most `PCI_NUM_RESOURCES` entries.
        index < bindings::PCI_NUM_RESOURCES
    }
}

impl<const SIZE: usize> Drop for Bar<SIZE> {
    fn drop(&mut self) {
        self.release();
    }
}

impl<const SIZE: usize> Deref for Bar<SIZE> {
    type Target = Io<SIZE>;

    fn deref(&self) -> &Self::Target {
        &self.io
    }
}

impl Device {
    /// Create a PCI Device instance from an existing `device::Device`.
    ///
    /// # Safety
    ///
    /// `dev` must be an `ARef<device::Device>` whose underlying `bindings::device` is a member of
    /// a `bindings::pci_dev`.
    pub unsafe fn from_dev(dev: ARef<device::Device>) -> Self {
        Self(dev)
    }

    fn as_raw(&self) -> *mut bindings::pci_dev {
        // SAFETY: By the type invariant `self.0.as_raw` is a pointer to the `struct device`
        // embedded in `struct pci_dev`.
        unsafe { container_of!(self.0.as_raw(), bindings::pci_dev, dev) as _ }
    }

    /// Enable memory resources for this device.
    pub fn enable_device_mem(&self) -> Result {
        // SAFETY: `self.as_raw` is guaranteed to be a pointer to a valid `struct pci_dev`.
        let ret = unsafe { bindings::pci_enable_device_mem(self.as_raw()) };
        if ret != 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(())
        }
    }

    /// Enable bus-mastering for this device.
    pub fn set_master(&self) {
        // SAFETY: `self.as_raw` is guaranteed to be a pointer to a valid `struct pci_dev`.
        unsafe { bindings::pci_set_master(self.as_raw()) };
    }

    /// enable MSI
    pub fn enable_msi(&self) {
        // SAFETY: Safe by the type invariants.
        unsafe { bindings::pci_enable_msi(self.as_raw()) };
    }

    /// Returns the size of the given PCI bar resource.
    pub fn resource_len(&self, bar: u32) -> Result<bindings::resource_size_t> {
        if !Bar::index_is_valid(bar) {
            return Err(EINVAL);
        }

        // SAFETY: Safe by the type invariant.
        Ok(unsafe { bindings::pci_resource_len(self.as_raw(), bar.try_into()?) })
    }

    /// Returns the size of the given PCI bar start
    pub fn resource_start(&self, bar: u32) -> Result<bindings::resource_size_t> {
        if !Bar::index_is_valid(bar) {
            return Err(EINVAL);
        }

        // SAFETY: Safe by the type invariant.
        Ok(unsafe { bindings::pci_resource_start(self.as_raw(), bar.try_into()?) })
    }

    /// Enable SRIOV virtual functions for this device
    pub fn enable_sriov(&self, num_vfs: i32) -> Result<i32> {
	Ok(unsafe { bindings::pci_enable_sriov(self.as_raw(), num_vfs) })
    }

    /// Disable SRIOV virtual functions for this device.
    pub fn disable_sriov(&self) -> Result {
	Ok(unsafe { bindings::pci_disable_sriov(self.as_raw()) })
    }

    /// Returns the 16-bit bus/device/function
    pub fn dev_id(&self) -> Result<u16> {
	Ok(unsafe { bindings::pci_dev_id(self.as_raw()) })
    }

    /// Mapps an entire PCI-BAR after performing a region-request on it. I/O operation bound checks
    /// can be performed on compile time for offsets (plus the requested type size) < SIZE.
    pub fn iomap_region_sized<const SIZE: usize>(
        &self,
        bar: u32,
        name: &CStr,
    ) -> Result<Devres<Bar<SIZE>>> {
        let bar = Bar::<SIZE>::new(self.clone(), bar, name)?;
        let devres = Devres::new(self.as_ref(), bar, GFP_KERNEL)?;

        Ok(devres)
    }

    /// Mapps an entire PCI-BAR after performing a region-request on it.
    pub fn iomap_region(&self, bar: u32, name: &CStr) -> Result<Devres<Bar>> {
        self.iomap_region_sized::<0>(bar, name)
    }

    /// Returns a new `ARef` of the base `device::Device`.
    pub fn as_dev(&self) -> ARef<device::Device> {
        self.0.clone()
    }

    /// Is this device a virtual function
    pub fn is_virtfn(&self) -> bool {
        let pdev = self.as_raw();
	unsafe { (*pdev).is_virtfn() == 1 }
    }

    pub fn device_vendor_id(&self) -> (u16, u16) {
        let pdev = self.as_raw();
	unsafe { ((*pdev).vendor, (*pdev).device) }
    }

    // TODO: check that all these &self methods use internal synchronization
    pub fn irq(&self) -> Option<u32> {
        let pdev = self.as_raw();
        let irq = unsafe { (*pdev).irq };
        if irq == 0 {
            None
        } else {
            Some(irq)
        }
    }

    /// Read a 32-bit value from configuration space
    pub fn read_config_dword(&self, offset: u32) -> Result<u32> {
	let pdev = self.as_raw();
	let mut val: u32 = 0;
	let ret = unsafe { bindings::pci_read_config_dword(pdev, offset as i32, &mut val) };

	if ret < 0 {
            Err(Error::from_errno(ret))
	} else {
	    Ok(val)
	}
    }

    /// Read a 16-bit value from configuration space
    pub fn read_config_word(&self, offset: u32) -> Result<u16> {
	let pdev = self.as_raw();
	let mut val: u16 = 0;
	let ret = unsafe { bindings::pci_read_config_word(pdev, offset as i32, &mut val) };

	if ret < 0 {
            Err(Error::from_errno(ret))
	} else {
	    Ok(val)
	}
    }

    /// Find a PCI extended capability
    pub fn find_ext_capability(&self, cap: i32) -> Result<u16> {
	let pdev = self.as_raw();

	let val = unsafe { bindings::pci_find_ext_capability(pdev, cap) };
	Ok(val)
    }

    pub fn alloc_irq_vectors(&self, min_vecs: u32, max_vecs: u32, flags: u32) -> Result<u32> {
        let ret = unsafe {
            bindings::pci_alloc_irq_vectors_affinity(
                self.as_raw(),
                min_vecs,
                max_vecs,
                flags,
                core::ptr::null_mut(),
            )
        };
        if ret < 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(ret as _)
        }
    }

    pub fn alloc_irq_vectors_affinity(
        &self,
        min_vecs: u32,
        max_vecs: u32,
        pre: u32,
        post: u32,
        flags: u32,
    ) -> Result<u32> {
        let mut affd = bindings::irq_affinity {
            pre_vectors: pre,
            post_vectors: post,
            ..bindings::irq_affinity::default()
        };

        let ret = unsafe {
            bindings::pci_alloc_irq_vectors_affinity(
                self.as_raw(),
                min_vecs,
                max_vecs,
                flags | bindings::PCI_IRQ_AFFINITY,
                &mut affd,
            )
        };
        if ret < 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(ret as _)
        }
    }

    pub fn free_irq_vectors(&self) {
        unsafe { bindings::pci_free_irq_vectors(self.as_raw()) };
    }

    pub fn request_irq<T: irq::Handler>(
        &self,
        index: u32,
        data: T::Data,
        name_args: fmt::Arguments<'_>,
    ) -> Result<irq::Registration<T>> {
        let ret = unsafe { bindings::pci_irq_vector(self.as_raw(), index) };
        if ret < 0 {
            return Err(Error::from_errno(ret));
        }
        crate::pr_info!("Setting up IRQ: {}\n", ret);

        irq::Registration::try_thread_new(ret as _, data, irq::flags::SHARED, name_args)
    }
}

impl AsRef<device::Device> for Device {
    fn as_ref(&self) -> &device::Device {
        &self.0
    }
}
