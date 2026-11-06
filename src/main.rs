#![no_std]
#![no_main]

extern crate alloc;

mod console;
mod screen;

use core::{
   ffi::c_void,
   panic::PanicInfo,
   ptr::{
      self,
      NonNull,
   },
   slice,
   sync::atomic::{
      AtomicPtr,
      Ordering,
   },
   time::Duration,
};

use uefi::{
   Handle,
   Status,
   boot::{
      OpenProtocolAttributes,
      OpenProtocolParams,
      SearchType,
      calculate_crc32,
      image_handle,
      install_protocol_interface,
      locate_handle_buffer,
      open_protocol,
      stall,
      test_protocol,
   },
   entry,
   println,
   proto::{
      console::gop::GraphicsOutput,
      device_path::DevicePath,
   },
   table::system_table_raw,
};
use uefi_raw::{
   protocol::console::{
      GraphicsOutputProtocol,
      SimpleTextOutputProtocol,
   },
   table::system::SystemTable,
};

use crate::{
   console::Console,
   screen::{
      Rotation,
      Screen,
   },
};

static FIRMWARE_CONSOLE: AtomicPtr<SimpleTextOutputProtocol> = AtomicPtr::new(ptr::null_mut());
static FIRMWARE_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

#[entry]
fn main() -> Status {
   // Returning ABORTED makes systemd-boot unload the driver quietly.
   let Some(screen) = find_screen(Rotation::BUILT) else {
      return Status::ABORTED;
   };

   let Some(console) = Console::leak(screen) else {
      return Status::ABORTED;
   };

   let protocol = console.protocol();

   // SAFETY: `protocol` points to a valid `SimpleTextOutputProtocol` leaked for
   // the duration of boot services.
   match unsafe {
      install_protocol_interface(None, &SimpleTextOutputProtocol::GUID, protocol.cast())
   } {
      Ok(handle) => swap_console(handle, protocol),
      Err(error) => error.status(),
   }
}

fn find_screen(rotation: Rotation) -> Option<Screen> {
   let handles =
      locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutputProtocol::GUID)).ok()?;
   let mut virtual_screen = None;

   for &handle in handles.iter() {
      let params = OpenProtocolParams {
         handle,
         agent: image_handle(),
         controller: None,
      };

      // SAFETY: `GetProtocol` does not take exclusive ownership, and the handle
      // stays valid while boot services are active.
      let Ok(mut gop) =
         (unsafe { open_protocol::<GraphicsOutput>(params, OpenProtocolAttributes::GetProtocol) })
      else {
         continue;
      };

      let raw = NonNull::from(&mut *gop).cast::<GraphicsOutputProtocol>();

      // SAFETY: `GraphicsOutput` is a `repr(transparent)` wrapper around
      // `GraphicsOutputProtocol`.
      let Some(screen) = (unsafe { Screen::new(raw, rotation) }) else {
         continue;
      };

      // ConSplitter installs a virtual GOP without a DevicePath, so prefer a
      // handle that has one.
      if test_protocol::<DevicePath>(params).unwrap_or(false) {
         return Some(screen);
      }

      virtual_screen.get_or_insert(screen);
   }

   virtual_screen
}

fn swap_console(handle: Handle, protocol: *mut SimpleTextOutputProtocol) -> Status {
   // SAFETY: the firmware starts drivers one at a time, so nothing else is
   // accessing the system table here.
   let Some(table) = system_table_raw().map(|mut raw| unsafe { raw.as_mut() }) else {
      return Status::NOT_READY;
   };

   let firmware = table.stdout;
   FIRMWARE_CONSOLE.store(firmware, Ordering::Relaxed);
   FIRMWARE_HANDLE.store(table.stdout_handle, Ordering::Relaxed);
   table.stdout_handle = handle.as_ptr();
   table.stdout = protocol;

   if table.stderr == firmware {
      table.stderr_handle = handle.as_ptr();
      table.stderr = protocol;
   }

   reseal(table)
}

fn reseal(table: &mut SystemTable) -> Status {
   let Ok(size) = usize::try_from(table.header.size) else {
      return Status::BAD_BUFFER_SIZE;
   };

   table.header.crc = 0;

   // SAFETY: `table.header.size` is the byte length of the `SystemTable`.
   let bytes = unsafe { slice::from_raw_parts((&raw const *table).cast::<u8>(), size) };

   match calculate_crc32(bytes) {
      Ok(crc) => {
         table.header.crc = crc;
         Status::SUCCESS
      },
      Err(error) => error.status(),
   }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
   let firmware = FIRMWARE_CONSOLE.load(Ordering::Relaxed);

   // SAFETY: the firmware starts drivers one at a time, so nothing else
   // touches the system table while this one panics.
   if let Some(table) = system_table_raw().map(|mut raw| unsafe { raw.as_mut() })
      && !firmware.is_null()
   {
      let handle = FIRMWARE_HANDLE.load(Ordering::Relaxed);

      if table.stderr == table.stdout {
         table.stderr_handle = handle;
         table.stderr = firmware;
      }

      table.stdout_handle = handle;
      table.stdout = firmware;
      let _ = reseal(table);
   }

   println!("gop-rotate: {info}");

   loop {
      stall(Duration::from_secs(1));
   }
}
