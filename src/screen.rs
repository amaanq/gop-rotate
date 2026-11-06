use alloc::vec::Vec;
use core::ptr::NonNull;

use uefi::Status;
use uefi_raw::protocol::console::{
   GraphicsOutputBltOperation,
   GraphicsOutputBltPixel,
   GraphicsOutputProtocol,
};

/// Clockwise turn from upright into the framebuffer. A panel the kernel reports
/// as right side up needs a quarter turn.
#[derive(Clone, Copy)]
pub enum Rotation {
   Quarter,
   Half,
   ThreeQuarters,
}

impl Rotation {
   pub const BUILT: Self = match option_env!("GOP_ROTATE_DEGREES") {
      None => Self::Quarter,
      Some(degrees) => {
         match degrees.as_bytes() {
            b"90" => Self::Quarter,
            b"180" => Self::Half,
            b"270" => Self::ThreeQuarters,
            _ => panic!("GOP_ROTATE_DEGREES must be 90, 180 or 270"),
         }
      },
   };

   const fn swaps_axes(self) -> bool {
      matches!(self, Self::Quarter | Self::ThreeQuarters)
   }
}

#[derive(Clone, Copy)]
pub struct Rect {
   pub x:      usize,
   pub y:      usize,
   pub width:  usize,
   pub height: usize,
}

pub struct Screen {
   gop:           NonNull<GraphicsOutputProtocol>,
   rotation:      Rotation,
   native_width:  usize,
   native_height: usize,
}

impl Screen {
   /// # Safety
   ///
   /// `gop` must point to a valid `GraphicsOutputProtocol` that remains live
   /// for the duration of boot services.
   pub unsafe fn new(gop: NonNull<GraphicsOutputProtocol>, rotation: Rotation) -> Option<Self> {
      // SAFETY: the caller guarantees `gop` is valid.
      let gop_mode = unsafe { gop.as_ref() }.mode;

      // SAFETY: firmware populates `mode` when a GOP mode is active.
      let info_ptr = unsafe { gop_mode.as_ref() }?.info;

      // SAFETY: firmware populates `info` for the active mode.
      let info = unsafe { info_ptr.as_ref() }?;
      let native_width = usize::try_from(info.horizontal_resolution).ok()?;
      let native_height = usize::try_from(info.vertical_resolution).ok()?;

      if native_width == 0 || native_height == 0 {
         return None;
      }

      if rotation.swaps_axes() && native_width >= native_height {
         return None;
      }

      Some(Self {
         gop,
         rotation,
         native_width,
         native_height,
      })
   }

   pub const fn width(&self) -> usize {
      if self.rotation.swaps_axes() {
         self.native_height
      } else {
         self.native_width
      }
   }

   pub const fn height(&self) -> usize {
      if self.rotation.swaps_axes() {
         self.native_width
      } else {
         self.native_height
      }
   }

   pub const fn bounds(&self) -> Rect {
      Rect {
         x:      0,
         y:      0,
         width:  self.width(),
         height: self.height(),
      }
   }

   const fn native_rect(&self, rect: Rect) -> Rect {
      match self.rotation {
         Rotation::Quarter => {
            Rect {
               x:      self.native_width - rect.y - rect.height,
               y:      rect.x,
               width:  rect.height,
               height: rect.width,
            }
         },
         Rotation::Half => {
            Rect {
               x:      self.native_width - rect.x - rect.width,
               y:      self.native_height - rect.y - rect.height,
               width:  rect.width,
               height: rect.height,
            }
         },
         Rotation::ThreeQuarters => {
            Rect {
               x:      rect.y,
               y:      self.native_height - rect.x - rect.width,
               width:  rect.height,
               height: rect.width,
            }
         },
      }
   }

   pub fn fill(&self, rect: Rect, color: GraphicsOutputBltPixel) -> Status {
      let mut pixel = color;

      // SAFETY: `BLT_VIDEO_FILL` only reads a single pixel from `buffer`.
      unsafe {
         self.blt(
            &raw mut pixel,
            GraphicsOutputBltOperation::BLT_VIDEO_FILL,
            self.native_rect(rect),
            0,
         )
      }
   }

   pub fn draw(
      &self,
      rect: Rect,
      pixels: &[GraphicsOutputBltPixel],
      scratch: &mut Vec<GraphicsOutputBltPixel>,
   ) -> Status {
      if rect.width == 0 || rect.height == 0 || pixels.len() != rect.width * rect.height {
         return Status::INVALID_PARAMETER;
      }

      let native = self.native_rect(rect);
      scratch.clear();
      scratch.resize(pixels.len(), GraphicsOutputBltPixel::default());

      for (row, line) in pixels.chunks_exact(rect.width).enumerate() {
         for (column, pixel) in line.iter().enumerate() {
            let (native_column, native_row) = match self.rotation {
               Rotation::Quarter => (rect.height - 1 - row, column),
               Rotation::Half => (rect.width - 1 - column, rect.height - 1 - row),
               Rotation::ThreeQuarters => (row, rect.width - 1 - column),
            };

            scratch[native_row * native.width + native_column] = *pixel;
         }
      }

      // SAFETY: `scratch` contains `native.width * native.height` pixels.
      unsafe {
         self.blt(
            scratch.as_mut_ptr(),
            GraphicsOutputBltOperation::BLT_BUFFER_TO_VIDEO,
            native,
            native.width * size_of::<GraphicsOutputBltPixel>(),
         )
      }
   }

   unsafe fn blt(
      &self,
      buffer: *mut GraphicsOutputBltPixel,
      operation: GraphicsOutputBltOperation,
      native: Rect,
      delta: usize,
   ) -> Status {
      // SAFETY: `self.gop` is valid per `Screen::new`.
      let blt = unsafe { self.gop.as_ref() }.blt;

      // SAFETY: the caller guarantees `buffer` is valid for `operation` and
      // `native`.
      unsafe {
         blt(
            self.gop.as_ptr(),
            buffer,
            operation,
            0,
            0,
            native.x,
            native.y,
            native.width,
            native.height,
            delta,
         )
      }
   }
}
