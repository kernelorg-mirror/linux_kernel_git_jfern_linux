// SPDX-License-Identifier: GPL-2.0

//! Abstractions for the PCI bus.
//!
//! C header: [`include/linux/pci.h`](srctree/include/linux/pci.h)

use crate::{
    bindings, container_of, device,
    device_id::{RawDeviceId, RawDeviceIdIndex},
    devres::Devres,
    driver,
    error::{from_result, to_result, Result},
    io::Io,
    io::IoRaw,
    str::CStr,
    types::{ARef, Opaque},
    ThisModule,
};
use core::{
    marker::PhantomData,
    ops::Deref,
    ptr::{addr_of_mut, NonNull},
};
use kernel::prelude::*;

/// PCI device class codes. Each entry contains the full 24-bit PCI
/// class code (base class in bits 23-16, subclass in bits 15-8,
/// programming interface in bits 7-0).
///
/// # Examples
///
/// ```
/// # use kernel::{device::Core, pci::{self, Class}, prelude::*};
/// fn probe_device(pdev: &pci::Device<Core>) -> Result<()> {
///     // Get the PCI class for this device
///     let pci_class = pdev.pci_class();
///     dev_info!(
///         pdev.as_ref(),
///         "Detected PCI class: (0x{:06x})\n",
///         pci_class.as_u32()
///     );
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Class(u32);

macro_rules! define_all_pci_classes {
    (
        $($variant:ident = $binding:expr,)+
    ) => {

        impl Class {
            $(
                #[allow(missing_docs)]
                pub const $variant: Self = Self(Self::to_24bit_class($binding));
            )+
        }

        /// Try to convert a raw 24-bit class code value to a `Class` enum.
        /// Returns `ENODEV` if the value doesn't match any known class.
        impl TryFrom<u32> for Class {
            type Error = Error;

            fn try_from(value: u32) -> Result<Self, Self::Error> {
                match value {
                    $(x if x == Self::$variant.0 => Ok(Self::$variant),)+
                    _ => Err(ENODEV),
                }
            }
        }
    };
}

impl Class {
    /// Match the full class code.
    pub const MASK_FULL: u32 = 0xffffff;

    /// Match the upper 16 bits of the class code (base class and subclass only).
    pub const MASK_CLASS_SUBCLASS: u32 = 0xffff00;

    /// Get the raw 24-bit class code value.
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    // Converts a PCI class constant to 24-bit format.
    //
    // Many device drivers use only the upper 16 bits (base class and subclass), but some
    // use the full 24 bits. In order to support both cases, store the class code as a 24-bit
    // value, where 16-bit values are shifted up 8 bits.
    const fn to_24bit_class(val: u32) -> u32 {
        if val > 0xFFFF {
            val
        } else {
            val << 8
        }
    }
}

define_all_pci_classes! {
    NOT_DEFINED                = bindings::PCI_CLASS_NOT_DEFINED,                // 0x000000
    NOT_DEFINED_VGA            = bindings::PCI_CLASS_NOT_DEFINED_VGA,            // 0x000100

    STORAGE_SCSI               = bindings::PCI_CLASS_STORAGE_SCSI,               // 0x010000
    STORAGE_IDE                = bindings::PCI_CLASS_STORAGE_IDE,                // 0x010100
    STORAGE_FLOPPY             = bindings::PCI_CLASS_STORAGE_FLOPPY,             // 0x010200
    STORAGE_IPI                = bindings::PCI_CLASS_STORAGE_IPI,                // 0x010300
    STORAGE_RAID               = bindings::PCI_CLASS_STORAGE_RAID,               // 0x010400
    STORAGE_SATA               = bindings::PCI_CLASS_STORAGE_SATA,               // 0x010600
    STORAGE_SATA_AHCI          = bindings::PCI_CLASS_STORAGE_SATA_AHCI,          // 0x010601
    STORAGE_SAS                = bindings::PCI_CLASS_STORAGE_SAS,                // 0x010700
    STORAGE_EXPRESS            = bindings::PCI_CLASS_STORAGE_EXPRESS,            // 0x010802
    STORAGE_OTHER              = bindings::PCI_CLASS_STORAGE_OTHER,              // 0x018000

    NETWORK_ETHERNET           = bindings::PCI_CLASS_NETWORK_ETHERNET,           // 0x020000
    NETWORK_TOKEN_RING         = bindings::PCI_CLASS_NETWORK_TOKEN_RING,         // 0x020100
    NETWORK_FDDI               = bindings::PCI_CLASS_NETWORK_FDDI,               // 0x020200
    NETWORK_ATM                = bindings::PCI_CLASS_NETWORK_ATM,                // 0x020300
    NETWORK_OTHER              = bindings::PCI_CLASS_NETWORK_OTHER,              // 0x028000

    DISPLAY_VGA                = bindings::PCI_CLASS_DISPLAY_VGA,                // 0x030000
    DISPLAY_XGA                = bindings::PCI_CLASS_DISPLAY_XGA,                // 0x030100
    DISPLAY_3D                 = bindings::PCI_CLASS_DISPLAY_3D,                 // 0x030200
    DISPLAY_OTHER              = bindings::PCI_CLASS_DISPLAY_OTHER,              // 0x038000

    MULTIMEDIA_VIDEO           = bindings::PCI_CLASS_MULTIMEDIA_VIDEO,           // 0x040000
    MULTIMEDIA_AUDIO           = bindings::PCI_CLASS_MULTIMEDIA_AUDIO,           // 0x040100
    MULTIMEDIA_PHONE           = bindings::PCI_CLASS_MULTIMEDIA_PHONE,           // 0x040200
    MULTIMEDIA_HD_AUDIO        = bindings::PCI_CLASS_MULTIMEDIA_HD_AUDIO,        // 0x040300
    MULTIMEDIA_OTHER           = bindings::PCI_CLASS_MULTIMEDIA_OTHER,           // 0x048000

    MEMORY_RAM                 = bindings::PCI_CLASS_MEMORY_RAM,                 // 0x050000
    MEMORY_FLASH               = bindings::PCI_CLASS_MEMORY_FLASH,               // 0x050100
    MEMORY_CXL                 = bindings::PCI_CLASS_MEMORY_CXL,                 // 0x050200
    MEMORY_OTHER               = bindings::PCI_CLASS_MEMORY_OTHER,               // 0x058000

    BRIDGE_HOST                = bindings::PCI_CLASS_BRIDGE_HOST,                // 0x060000
    BRIDGE_ISA                 = bindings::PCI_CLASS_BRIDGE_ISA,                 // 0x060100
    BRIDGE_EISA                = bindings::PCI_CLASS_BRIDGE_EISA,                // 0x060200
    BRIDGE_MC                  = bindings::PCI_CLASS_BRIDGE_MC,                  // 0x060300
    BRIDGE_PCI_NORMAL          = bindings::PCI_CLASS_BRIDGE_PCI_NORMAL,          // 0x060400
    BRIDGE_PCI_SUBTRACTIVE     = bindings::PCI_CLASS_BRIDGE_PCI_SUBTRACTIVE,     // 0x060401
    BRIDGE_PCMCIA              = bindings::PCI_CLASS_BRIDGE_PCMCIA,              // 0x060500
    BRIDGE_NUBUS               = bindings::PCI_CLASS_BRIDGE_NUBUS,               // 0x060600
    BRIDGE_CARDBUS             = bindings::PCI_CLASS_BRIDGE_CARDBUS,             // 0x060700
    BRIDGE_RACEWAY             = bindings::PCI_CLASS_BRIDGE_RACEWAY,             // 0x060800
    BRIDGE_OTHER               = bindings::PCI_CLASS_BRIDGE_OTHER,               // 0x068000

    COMMUNICATION_SERIAL       = bindings::PCI_CLASS_COMMUNICATION_SERIAL,       // 0x070000
    COMMUNICATION_PARALLEL     = bindings::PCI_CLASS_COMMUNICATION_PARALLEL,     // 0x070100
    COMMUNICATION_MULTISERIAL  = bindings::PCI_CLASS_COMMUNICATION_MULTISERIAL,  // 0x070200
    COMMUNICATION_MODEM        = bindings::PCI_CLASS_COMMUNICATION_MODEM,        // 0x070300
    COMMUNICATION_OTHER        = bindings::PCI_CLASS_COMMUNICATION_OTHER,        // 0x078000

    SYSTEM_PIC                 = bindings::PCI_CLASS_SYSTEM_PIC,                 // 0x080000
    SYSTEM_PIC_IOAPIC          = bindings::PCI_CLASS_SYSTEM_PIC_IOAPIC,          // 0x080010
    SYSTEM_PIC_IOXAPIC         = bindings::PCI_CLASS_SYSTEM_PIC_IOXAPIC,         // 0x080020
    SYSTEM_DMA                 = bindings::PCI_CLASS_SYSTEM_DMA,                 // 0x080100
    SYSTEM_TIMER               = bindings::PCI_CLASS_SYSTEM_TIMER,               // 0x080200
    SYSTEM_RTC                 = bindings::PCI_CLASS_SYSTEM_RTC,                 // 0x080300
    SYSTEM_PCI_HOTPLUG         = bindings::PCI_CLASS_SYSTEM_PCI_HOTPLUG,         // 0x080400
    SYSTEM_SDHCI               = bindings::PCI_CLASS_SYSTEM_SDHCI,               // 0x080500
    SYSTEM_RCEC                = bindings::PCI_CLASS_SYSTEM_RCEC,                // 0x080700
    SYSTEM_OTHER               = bindings::PCI_CLASS_SYSTEM_OTHER,               // 0x088000

    INPUT_KEYBOARD             = bindings::PCI_CLASS_INPUT_KEYBOARD,             // 0x090000
    INPUT_PEN                  = bindings::PCI_CLASS_INPUT_PEN,                  // 0x090100
    INPUT_MOUSE                = bindings::PCI_CLASS_INPUT_MOUSE,                // 0x090200
    INPUT_SCANNER              = bindings::PCI_CLASS_INPUT_SCANNER,              // 0x090300
    INPUT_GAMEPORT             = bindings::PCI_CLASS_INPUT_GAMEPORT,             // 0x090400
    INPUT_OTHER                = bindings::PCI_CLASS_INPUT_OTHER,                // 0x098000

    DOCKING_GENERIC            = bindings::PCI_CLASS_DOCKING_GENERIC,            // 0x0a0000
    DOCKING_OTHER              = bindings::PCI_CLASS_DOCKING_OTHER,              // 0x0a8000

    PROCESSOR_386              = bindings::PCI_CLASS_PROCESSOR_386,              // 0x0b0000
    PROCESSOR_486              = bindings::PCI_CLASS_PROCESSOR_486,              // 0x0b0100
    PROCESSOR_PENTIUM          = bindings::PCI_CLASS_PROCESSOR_PENTIUM,          // 0x0b0200
    PROCESSOR_ALPHA            = bindings::PCI_CLASS_PROCESSOR_ALPHA,            // 0x0b1000
    PROCESSOR_POWERPC          = bindings::PCI_CLASS_PROCESSOR_POWERPC,          // 0x0b2000
    PROCESSOR_MIPS             = bindings::PCI_CLASS_PROCESSOR_MIPS,             // 0x0b3000
    PROCESSOR_CO               = bindings::PCI_CLASS_PROCESSOR_CO,               // 0x0b4000

    SERIAL_FIREWIRE            = bindings::PCI_CLASS_SERIAL_FIREWIRE,            // 0x0c0000
    SERIAL_FIREWIRE_OHCI       = bindings::PCI_CLASS_SERIAL_FIREWIRE_OHCI,       // 0x0c0010
    SERIAL_ACCESS              = bindings::PCI_CLASS_SERIAL_ACCESS,              // 0x0c0100
    SERIAL_SSA                 = bindings::PCI_CLASS_SERIAL_SSA,                 // 0x0c0200
    SERIAL_USB_UHCI            = bindings::PCI_CLASS_SERIAL_USB_UHCI,            // 0x0c0300
    SERIAL_USB_OHCI            = bindings::PCI_CLASS_SERIAL_USB_OHCI,            // 0x0c0310
    SERIAL_USB_EHCI            = bindings::PCI_CLASS_SERIAL_USB_EHCI,            // 0x0c0320
    SERIAL_USB_XHCI            = bindings::PCI_CLASS_SERIAL_USB_XHCI,            // 0x0c0330
    SERIAL_USB_CDNS            = bindings::PCI_CLASS_SERIAL_USB_CDNS,            // 0x0c0380
    SERIAL_USB_DEVICE          = bindings::PCI_CLASS_SERIAL_USB_DEVICE,          // 0x0c03fe
    SERIAL_FIBER               = bindings::PCI_CLASS_SERIAL_FIBER,               // 0x0c0400
    SERIAL_SMBUS               = bindings::PCI_CLASS_SERIAL_SMBUS,               // 0x0c0500
    SERIAL_IPMI_SMIC           = bindings::PCI_CLASS_SERIAL_IPMI_SMIC,           // 0x0c0700
    SERIAL_IPMI_KCS            = bindings::PCI_CLASS_SERIAL_IPMI_KCS,            // 0x0c0701
    SERIAL_IPMI_BT             = bindings::PCI_CLASS_SERIAL_IPMI_BT,             // 0x0c0702

    WIRELESS_RF_CONTROLLER     = bindings::PCI_CLASS_WIRELESS_RF_CONTROLLER,     // 0x0d1000
    WIRELESS_WHCI              = bindings::PCI_CLASS_WIRELESS_WHCI,              // 0x0d1010

    INTELLIGENT_I2O            = bindings::PCI_CLASS_INTELLIGENT_I2O,            // 0x0e0000

    SATELLITE_TV               = bindings::PCI_CLASS_SATELLITE_TV,               // 0x0f0000
    SATELLITE_AUDIO            = bindings::PCI_CLASS_SATELLITE_AUDIO,            // 0x0f0100
    SATELLITE_VOICE            = bindings::PCI_CLASS_SATELLITE_VOICE,            // 0x0f0300
    SATELLITE_DATA             = bindings::PCI_CLASS_SATELLITE_DATA,             // 0x0f0400

    CRYPT_NETWORK              = bindings::PCI_CLASS_CRYPT_NETWORK,              // 0x100000
    CRYPT_ENTERTAINMENT        = bindings::PCI_CLASS_CRYPT_ENTERTAINMENT,        // 0x100100
    CRYPT_OTHER                = bindings::PCI_CLASS_CRYPT_OTHER,                // 0x108000

    SP_DPIO                    = bindings::PCI_CLASS_SP_DPIO,                    // 0x110000
    SP_OTHER                   = bindings::PCI_CLASS_SP_OTHER,                   // 0x118000

    ACCELERATOR_PROCESSING     = bindings::PCI_CLASS_ACCELERATOR_PROCESSING,     // 0x120000

    OTHERS                     = bindings::PCI_CLASS_OTHERS,                     // 0xff0000
}

/// An adapter for the registration of PCI drivers.
pub struct Adapter<T: Driver>(T);

// SAFETY: A call to `unregister` for a given instance of `RegType` is guaranteed to be valid if
// a preceding call to `register` has been successful.
unsafe impl<T: Driver + 'static> driver::RegistrationOps for Adapter<T> {
    type RegType = bindings::pci_driver;

    unsafe fn register(
        pdrv: &Opaque<Self::RegType>,
        name: &'static CStr,
        module: &'static ThisModule,
    ) -> Result {
        // SAFETY: It's safe to set the fields of `struct pci_driver` on initialization.
        unsafe {
            (*pdrv.get()).name = name.as_char_ptr();
            (*pdrv.get()).probe = Some(Self::probe_callback);
            (*pdrv.get()).remove = Some(Self::remove_callback);
            (*pdrv.get()).id_table = T::ID_TABLE.as_ptr();
        }

        // SAFETY: `pdrv` is guaranteed to be a valid `RegType`.
        to_result(unsafe {
            bindings::__pci_register_driver(pdrv.get(), module.0, name.as_char_ptr())
        })
    }

    unsafe fn unregister(pdrv: &Opaque<Self::RegType>) {
        // SAFETY: `pdrv` is guaranteed to be a valid `RegType`.
        unsafe { bindings::pci_unregister_driver(pdrv.get()) }
    }
}

impl<T: Driver + 'static> Adapter<T> {
    extern "C" fn probe_callback(
        pdev: *mut bindings::pci_dev,
        id: *const bindings::pci_device_id,
    ) -> kernel::ffi::c_int {
        // SAFETY: The PCI bus only ever calls the probe callback with a valid pointer to a
        // `struct pci_dev`.
        //
        // INVARIANT: `pdev` is valid for the duration of `probe_callback()`.
        let pdev = unsafe { &*pdev.cast::<Device<device::CoreInternal>>() };

        // SAFETY: `DeviceId` is a `#[repr(transparent)]` wrapper of `struct pci_device_id` and
        // does not add additional invariants, so it's safe to transmute.
        let id = unsafe { &*id.cast::<DeviceId>() };
        let info = T::ID_TABLE.info(id.index());

        from_result(|| {
            let data = T::probe(pdev, info)?;

            pdev.as_ref().set_drvdata(data);
            Ok(0)
        })
    }

    extern "C" fn remove_callback(pdev: *mut bindings::pci_dev) {
        // SAFETY: The PCI bus only ever calls the remove callback with a valid pointer to a
        // `struct pci_dev`.
        //
        // INVARIANT: `pdev` is valid for the duration of `remove_callback()`.
        let pdev = unsafe { &*pdev.cast::<Device<device::CoreInternal>>() };

        // SAFETY: `remove_callback` is only ever called after a successful call to
        // `probe_callback`, hence it's guaranteed that `Device::set_drvdata()` has been called
        // and stored a `Pin<KBox<T>>`.
        let data = unsafe { pdev.as_ref().drvdata_obtain::<Pin<KBox<T>>>() };

        T::unbind(pdev, data.as_ref());
    }
}

/// Declares a kernel module that exposes a single PCI driver.
///
/// # Examples
///
///```ignore
/// kernel::module_pci_driver! {
///     type: MyDriver,
///     name: "Module name",
///     authors: ["Author name"],
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

/// Abstraction for the PCI device ID structure ([`struct pci_device_id`]).
///
/// [`struct pci_device_id`]: https://docs.kernel.org/PCI/pci.html#c.pci_device_id
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct DeviceId(bindings::pci_device_id);

impl DeviceId {
    const PCI_ANY_ID: u32 = !0;

    /// Equivalent to C's `PCI_DEVICE` macro.
    ///
    /// Create a new `pci::DeviceId` from a vendor and device ID number.
    pub const fn from_id(vendor: u32, device: u32) -> Self {
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

    /// Equivalent to C's `PCI_DEVICE_CLASS` macro.
    ///
    /// Create a new `pci::DeviceId` from a class number and mask.
    pub const fn from_class(class: u32, class_mask: u32) -> Self {
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

    /// Create a new `pci::DeviceId` from a class number, mask, and specific vendor.
    ///
    /// This is more targeted than [`DeviceId::from_class`]: in addition to matching by Vendor, it
    /// also matches the PCI Class (up to the entire 24 bits, depending on the mask).
    pub const fn from_class_and_vendor(class: Class, class_mask: u32, vendor: u32) -> Self {
        Self(bindings::pci_device_id {
            vendor,
            device: DeviceId::PCI_ANY_ID,
            subvendor: DeviceId::PCI_ANY_ID,
            subdevice: DeviceId::PCI_ANY_ID,
            class: class.as_u32(),
            class_mask,
            driver_data: 0,
            override_only: 0,
        })
    }
}

// SAFETY: `DeviceId` is a `#[repr(transparent)]` wrapper of `pci_device_id` and does not add
// additional invariants, so it's safe to transmute to `RawType`.
unsafe impl RawDeviceId for DeviceId {
    type RawType = bindings::pci_device_id;
}

// SAFETY: `DRIVER_DATA_OFFSET` is the offset to the `driver_data` field.
unsafe impl RawDeviceIdIndex for DeviceId {
    const DRIVER_DATA_OFFSET: usize = core::mem::offset_of!(bindings::pci_device_id, driver_data);

    fn index(&self) -> usize {
        self.0.driver_data
    }
}

/// `IdTable` type for PCI.
pub type IdTable<T> = &'static dyn kernel::device_id::IdTable<DeviceId, T>;

/// Create a PCI `IdTable` with its alias for modpost.
#[macro_export]
macro_rules! pci_device_table {
    ($table_name:ident, $module_table_name:ident, $id_info_type: ty, $table_data: expr) => {
        const $table_name: $crate::device_id::IdArray<
            $crate::pci::DeviceId,
            $id_info_type,
            { $table_data.len() },
        > = $crate::device_id::IdArray::new($table_data);

        $crate::module_device_table!("pci", $module_table_name, $table_name);
    };
}

/// The PCI driver trait.
///
/// # Examples
///
///```
/// # use kernel::{bindings, device::Core, pci};
///
/// struct MyDriver;
///
/// kernel::pci_device_table!(
///     PCI_TABLE,
///     MODULE_PCI_TABLE,
///     <MyDriver as pci::Driver>::IdInfo,
///     [
///         (
///             pci::DeviceId::from_id(bindings::PCI_VENDOR_ID_REDHAT, bindings::PCI_ANY_ID as u32),
///             (),
///         )
///     ]
/// );
///
/// impl pci::Driver for MyDriver {
///     type IdInfo = ();
///     const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;
///
///     fn probe(
///         _pdev: &pci::Device<Core>,
///         _id_info: &Self::IdInfo,
///     ) -> Result<Pin<KBox<Self>>> {
///         Err(ENODEV)
///     }
/// }
///```
/// Drivers must implement this trait in order to get a PCI driver registered. Please refer to the
/// `Adapter` documentation for an example.
pub trait Driver: Send {
    /// The type holding information about each device id supported by the driver.
    // TODO: Use `associated_type_defaults` once stabilized:
    //
    // ```
    // type IdInfo: 'static = ();
    // ```
    type IdInfo: 'static;

    /// The table of device ids supported by the driver.
    const ID_TABLE: IdTable<Self::IdInfo>;

    /// PCI driver probe.
    ///
    /// Called when a new platform device is added or discovered.
    /// Implementers should attempt to initialize the device here.
    fn probe(dev: &Device<device::Core>, id_info: &Self::IdInfo) -> Result<Pin<KBox<Self>>>;

    /// Platform driver unbind.
    ///
    /// Called when a [`Device`] is unbound from its bound [`Driver`]. Implementing this callback
    /// is optional.
    ///
    /// This callback serves as a place for drivers to perform teardown operations that require a
    /// `&Device<Core>` or `&Device<Bound>` reference. For instance, drivers may try to perform I/O
    /// operations to gracefully tear down the device.
    ///
    /// Otherwise, release operations for driver resources should be performed in `Self::drop`.
    fn unbind(dev: &Device<device::Core>, this: Pin<&Self>) {
        let _ = (dev, this);
    }
}

/// The PCI device representation.
///
/// This structure represents the Rust abstraction for a C `struct pci_dev`. The implementation
/// abstracts the usage of an already existing C `struct pci_dev` within Rust code that we get
/// passed from the C side.
///
/// # Invariants
///
/// A [`Device`] instance represents a valid `struct pci_dev` created by the C portion of the
/// kernel.
#[repr(transparent)]
pub struct Device<Ctx: device::DeviceContext = device::Normal>(
    Opaque<bindings::pci_dev>,
    PhantomData<Ctx>,
);

/// A PCI BAR to perform I/O-Operations on.
///
/// # Invariants
///
/// `Bar` always holds an `IoRaw` inststance that holds a valid pointer to the start of the I/O
/// memory mapped PCI bar and its size.
pub struct Bar<const SIZE: usize = 0> {
    pdev: ARef<Device>,
    io: IoRaw<SIZE>,
    num: i32,
}

impl<const SIZE: usize> Bar<SIZE> {
    fn new(pdev: &Device, num: u32, name: &CStr) -> Result<Self> {
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

        let io = match IoRaw::new(ioptr, len as usize) {
            Ok(io) => io,
            Err(err) => {
                // SAFETY:
                // `pdev` is valid by the invariants of `Device`.
                // `ioptr` is guaranteed to be the start of a valid I/O mapped memory region.
                // `num` is checked for validity by a previous call to `Device::resource_len`.
                unsafe { Self::do_release(pdev, ioptr, num) };
                return Err(err);
            }
        };

        Ok(Bar {
            pdev: pdev.into(),
            io,
            num,
        })
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
            bindings::pci_iounmap(pdev.as_raw(), ioptr as *mut kernel::ffi::c_void);
            bindings::pci_release_region(pdev.as_raw(), num);
        }
    }

    fn release(&self) {
        // SAFETY: The safety requirements are guaranteed by the type invariant of `self.pdev`.
        unsafe { Self::do_release(&self.pdev, self.io.addr(), self.num) };
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
        // SAFETY: By the type invariant of `Self`, the MMIO range in `self.io` is properly mapped.
        unsafe { Io::from_raw(&self.io) }
    }
}

impl<Ctx: device::DeviceContext> Device<Ctx> {
    fn as_raw(&self) -> *mut bindings::pci_dev {
        self.0.get()
    }
}

impl Device {
    /// Returns the PCI vendor ID.
    pub fn vendor_id(&self) -> u16 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).vendor }
    }

    /// Returns the PCI device ID.
    pub fn device_id(&self) -> u16 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).device }
    }

    /// Returns the PCI revision ID.
    pub fn revision_id(&self) -> u8 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).revision }
    }

    /// Returns the PCI bus device/function.
    pub fn dev_id(&self) -> u16 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { bindings::pci_dev_id(self.as_raw()) }
    }

    /// Returns the PCI subsystem vendor ID.
    pub fn subsystem_vendor_id(&self) -> u16 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).subsystem_vendor }
    }

    /// Returns the PCI subsystem device ID.
    pub fn subsystem_device_id(&self) -> u16 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).subsystem_device }
    }

    /// Returns the start of the given PCI bar resource.
    pub fn resource_start(&self, bar: u32) -> Result<bindings::resource_size_t> {
        if !Bar::index_is_valid(bar) {
            return Err(EINVAL);
        }

        // SAFETY:
        // - `bar` is a valid bar number, as guaranteed by the above call to `Bar::index_is_valid`,
        // - by its type invariant `self.as_raw` is always a valid pointer to a `struct pci_dev`.
        Ok(unsafe { bindings::pci_resource_start(self.as_raw(), bar.try_into()?) })
    }

    /// Returns the size of the given PCI bar resource.
    pub fn resource_len(&self, bar: u32) -> Result<bindings::resource_size_t> {
        if !Bar::index_is_valid(bar) {
            return Err(EINVAL);
        }

        // SAFETY:
        // - `bar` is a valid bar number, as guaranteed by the above call to `Bar::index_is_valid`,
        // - by its type invariant `self.as_raw` is always a valid pointer to a `struct pci_dev`.
        Ok(unsafe { bindings::pci_resource_len(self.as_raw(), bar.try_into()?) })
    }

    /// Returns the full 24-bit PCI class code as stored in hardware.
    /// This includes base class, subclass, and programming interface.
    pub fn pci_class_code_raw(&self) -> u32 {
        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).class }
    }

    /// Returns the PCI class as a `Class` struct.
    pub fn pci_class(&self) -> Class {
        Class(self.pci_class_code_raw())
    }
}

impl Device<device::Bound> {
    /// Mapps an entire PCI-BAR after performing a region-request on it. I/O operation bound checks
    /// can be performed on compile time for offsets (plus the requested type size) < SIZE.
    pub fn iomap_region_sized<'a, const SIZE: usize>(
        &'a self,
        bar: u32,
        name: &'a CStr,
    ) -> impl PinInit<Devres<Bar<SIZE>>, Error> + 'a {
        Devres::new(self.as_ref(), Bar::<SIZE>::new(self, bar, name))
    }

    /// Mapps an entire PCI-BAR after performing a region-request on it.
    pub fn iomap_region<'a>(
        &'a self,
        bar: u32,
        name: &'a CStr,
    ) -> impl PinInit<Devres<Bar>, Error> + 'a {
        self.iomap_region_sized::<0>(bar, name)
    }
}

impl Device<device::Core> {
    /// Enable memory resources for this device.
    pub fn enable_device_mem(&self) -> Result {
        // SAFETY: `self.as_raw` is guaranteed to be a pointer to a valid `struct pci_dev`.
        to_result(unsafe { bindings::pci_enable_device_mem(self.as_raw()) })
    }

    /// Enable bus-mastering for this device.
    pub fn set_master(&self) {
        // SAFETY: `self.as_raw` is guaranteed to be a pointer to a valid `struct pci_dev`.
        unsafe { bindings::pci_set_master(self.as_raw()) };
    }
}

// SAFETY: `Device` is a transparent wrapper of a type that doesn't depend on `Device`'s generic
// argument.
kernel::impl_device_context_deref!(unsafe { Device });
kernel::impl_device_context_into_aref!(Device);

impl crate::dma::Device for Device<device::Core> {}

// SAFETY: Instances of `Device` are always reference-counted.
unsafe impl crate::types::AlwaysRefCounted for Device {
    fn inc_ref(&self) {
        // SAFETY: The existence of a shared reference guarantees that the refcount is non-zero.
        unsafe { bindings::pci_dev_get(self.as_raw()) };
    }

    unsafe fn dec_ref(obj: NonNull<Self>) {
        // SAFETY: The safety requirements guarantee that the refcount is non-zero.
        unsafe { bindings::pci_dev_put(obj.cast().as_ptr()) }
    }
}

impl<Ctx: device::DeviceContext> AsRef<device::Device<Ctx>> for Device<Ctx> {
    fn as_ref(&self) -> &device::Device<Ctx> {
        // SAFETY: By the type invariant of `Self`, `self.as_raw()` is a pointer to a valid
        // `struct pci_dev`.
        let dev = unsafe { addr_of_mut!((*self.as_raw()).dev) };

        // SAFETY: `dev` points to a valid `struct device`.
        unsafe { device::Device::from_raw(dev) }
    }
}

impl<Ctx: device::DeviceContext> TryFrom<&device::Device<Ctx>> for &Device<Ctx> {
    type Error = kernel::error::Error;

    fn try_from(dev: &device::Device<Ctx>) -> Result<Self, Self::Error> {
        // SAFETY: By the type invariant of `Device`, `dev.as_raw()` is a valid pointer to a
        // `struct device`.
        if !unsafe { bindings::dev_is_pci(dev.as_raw()) } {
            return Err(EINVAL);
        }

        // SAFETY: We've just verified that the bus type of `dev` equals `bindings::pci_bus_type`,
        // hence `dev` must be embedded in a valid `struct pci_dev` as guaranteed by the
        // corresponding C code.
        let pdev = unsafe { container_of!(dev.as_raw(), bindings::pci_dev, dev) };

        // SAFETY: `pdev` is a valid pointer to a `struct pci_dev`.
        Ok(unsafe { &*pdev.cast() })
    }
}

// SAFETY: A `Device` is always reference-counted and can be released from any thread.
unsafe impl Send for Device {}

// SAFETY: `Device` can be shared among threads because all methods of `Device`
// (i.e. `Device<Normal>) are thread safe.
unsafe impl Sync for Device {}
