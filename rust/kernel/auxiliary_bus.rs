use crate::{
    bindings, container_of, device,
    device_id::RawDeviceId,
    driver,
    error::{to_result, Result},
    str::CStr,
    types::{ARef, ForeignOwnable, Opaque},
    ThisModule,
};
use core::ptr::addr_of_mut;
use core::marker::PhantomPinned;
use kernel::prelude::*;

/// An adapter for the registration of auxiliary drivers.
pub struct Adapter<T: Driver>(T);

impl<T: Driver + 'static> driver::RegistrationOps for Adapter<T> {
    type RegType = bindings::auxiliary_driver;

    fn register(
        auxdrv: &mut Self::RegType,
        name: &'static CStr,
        module: &'static ThisModule,
    ) -> Result {
	auxdrv.name = name.as_char_ptr();
	auxdrv.probe = Some(Self::probe_callback);
	auxdrv.remove = Some(Self::remove_callback);
	auxdrv.id_table = T::ID_TABLE.as_ptr();
	to_result(unsafe {
	        bindings::__auxiliary_driver_register(auxdrv as _, module.0, name.as_char_ptr())
        })
    }

    fn unregister(auxdrv: &mut Self::RegType) {
        unsafe { bindings::auxiliary_driver_unregister(auxdrv) }
    }
}

impl<T: Driver + 'static> Adapter<T> {
    extern "C" fn probe_callback(
	auxdev: *mut bindings::auxiliary_device,
	id: *const bindings::auxiliary_device_id
    ) -> core::ffi::c_int {
	let dev = unsafe { device::Device::from_raw(&mut (*auxdev).dev) };
	let mut auxdev = unsafe { Device::from_dev(dev) };

	let index = unsafe { (*id).driver_data };
	let raw_id = T::ID_TABLE.id(index as _);

	let id = unsafe {
            core::mem::transmute::<&<DeviceId as RawDeviceId>::RawType, &DeviceId>(raw_id)
        };
        let info = T::ID_TABLE.info(index as _);

        match T::probe(&mut auxdev, id, info) {
            Ok(data) => {
                // Let the `struct pci_dev` own a reference of the driver's private data.
                // SAFETY: By the type invariant `pdev.as_raw` returns a valid pointer to a
                // `struct pci_dev`.
                unsafe { bindings::auxiliary_set_drvdata(auxdev.as_raw(), data.into_foreign() as _) };
            }
            Err(err) => return Error::to_errno(err),
        }
	0
    }

    extern "C" fn remove_callback(auxdev: *mut bindings::auxiliary_device) {
	let _ = unsafe {
	    let ptr = bindings::auxiliary_get_drvdata(auxdev);

	    KBox::<T>::from_foreign(ptr)
	};
    }
}

#[derive(Clone)]
pub struct Device(ARef<device::Device>);

pub trait Driver {
    type IdInfo: 'static;

    /// The table of device ids supported by the driver.
    const ID_TABLE: IdTable<Self::IdInfo>;
    fn probe(dev: &mut Device, id: &DeviceId, id_info: &Self::IdInfo) -> Result<Pin<KBox<Self>>>;
}

pub struct DeviceId(bindings::auxiliary_device_id);

unsafe impl RawDeviceId for DeviceId {
    type RawType = bindings::auxiliary_device_id;

    const DRIVER_DATA_OFFSET: usize = core::mem::offset_of!(bindings::auxiliary_device_id, driver_data);
}

/// IdTable type for auxiliary bus
pub type IdTable<T> = &'static dyn kernel::device_id::IdTable<DeviceId, T>;


impl Device {
    pub unsafe fn from_dev(dev: ARef<device::Device>) -> Self {
        Self(dev)
    }

    fn as_raw(&self) -> *mut bindings::auxiliary_device {
        // SAFETY: By the type invariant `self.0.as_raw` is a pointer to the `struct device`
        // embedded in `struct pci_dev`.
        unsafe { container_of!(self.0.as_raw(), bindings::auxiliary_device, dev) as _ }
    }
}

#[pin_data(PinnedDrop)]
pub struct RawDevice {
    #[pin]
    auxdev: Opaque<bindings::auxiliary_device>,
    #[pin]
    _p: PhantomPinned,
}

impl RawDevice {
    pub fn new(parent: ARef<device::Device>,
	       release: Option<unsafe extern "C" fn(*mut bindings::device)>,
	       name: &'static CStr,
	       id: u32,
	       modname: &'static CStr) -> impl PinInit<Self, Error> {
	unsafe {
	    init::pin_init_from_closure(move |slot: *mut Self| {
		let auxptr = Opaque::raw_get(addr_of_mut!((*slot).auxdev));

		::core::ptr::write_bytes(auxptr, 0, 1);
		addr_of_mut!((*auxptr).dev.parent).write(parent.as_raw());
		addr_of_mut!((*auxptr).dev.release).write(release);
		addr_of_mut!((*auxptr).name).write(name.as_char_ptr());
		addr_of_mut!((*auxptr).id).write(id);

		bindings::auxiliary_device_init(auxptr);

		let err = bindings::__auxiliary_device_add(auxptr, modname.as_char_ptr());
		if err != 0 {
		    bindings::auxiliary_device_uninit(auxptr);
		    return Err(Error::from_errno(err));
		}
		Ok(())
	    })
	}
    }
}

#[pinned_drop]
impl PinnedDrop for RawDevice {
    fn drop(self: Pin<&mut Self>) {
	unsafe {
	    let auxptr = self.auxdev.get();
	    bindings::auxiliary_device_delete(auxptr);
	    bindings::auxiliary_device_uninit(auxptr);
	}
    }
}
