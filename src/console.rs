use alloc::{
   boxed::Box,
   vec,
   vec::Vec,
};
use core::ptr;

use noto_sans_mono_bitmap::{
   FontWeight,
   RasterHeight,
   RasterizedChar,
   get_raster,
   get_raster_width,
};
use uefi::{
   CStr16,
   Status,
};
use uefi_raw::{
   Boolean,
   Char16,
   protocol::console::{
      GraphicsOutputBltPixel,
      SimpleTextOutputMode,
      SimpleTextOutputProtocol,
   },
};

use crate::screen::{
   Rect,
   Screen,
};

const WEIGHT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size32;
const GLYPH_WIDTH: usize = get_raster_width(WEIGHT, SIZE);
const GLYPH_HEIGHT: usize = SIZE.val();

// UEFI requires mode 0 to be 80x25 and reserves mode 1 for 80x50.
const MODE_COUNT: usize = 3;
const DEFAULT_MODE: usize = 2;

const DEFAULT_ATTRIBUTE: u8 = 0x07;

const fn rgb(red: u8, green: u8, blue: u8) -> GraphicsOutputBltPixel {
   GraphicsOutputBltPixel {
      blue,
      green,
      red,
      reserved: 0,
   }
}

// Standard 16-color EFI palette from EDK2's GraphicsConsoleDxe.
const PALETTE: [GraphicsOutputBltPixel; 16] = [
   rgb(0x00, 0x00, 0x00),
   rgb(0x00, 0x00, 0x98),
   rgb(0x00, 0x98, 0x00),
   rgb(0x00, 0x98, 0x98),
   rgb(0x98, 0x00, 0x00),
   rgb(0x98, 0x00, 0x98),
   rgb(0x98, 0x98, 0x00),
   rgb(0x98, 0x98, 0x98),
   rgb(0x30, 0x30, 0x30),
   rgb(0x00, 0x00, 0xFF),
   rgb(0x00, 0xFF, 0x00),
   rgb(0x00, 0xFF, 0xFF),
   rgb(0xFF, 0x00, 0x00),
   rgb(0xFF, 0x00, 0xFF),
   rgb(0xFF, 0xFF, 0x00),
   rgb(0xFF, 0xFF, 0xFF),
];

fn foreground(attribute: u8) -> GraphicsOutputBltPixel {
   PALETTE[usize::from(attribute & 0x0F)]
}

fn background(attribute: u8) -> GraphicsOutputBltPixel {
   PALETTE[usize::from((attribute >> 4) & 0x07)]
}

#[expect(
   clippy::integer_division,
   clippy::integer_division_remainder_used,
   reason = "alpha blending 8-bit channels"
)]
fn blend(
   back: GraphicsOutputBltPixel,
   fore: GraphicsOutputBltPixel,
   intensity: u8,
) -> GraphicsOutputBltPixel {
   let weight = u32::from(intensity);
   let mix = |under: u8, over: u8| {
      let mixed = (u32::from(under) * (255 - weight) + u32::from(over) * weight) / 255;
      u8::try_from(mixed).unwrap_or(u8::MAX)
   };

   rgb(
      mix(back.red, fore.red),
      mix(back.green, fore.green),
      mix(back.blue, fore.blue),
   )
}

const fn raster(glyph: char) -> Option<RasterizedChar> {
   get_raster(glyph, WEIGHT, SIZE)
}

fn mirror(value: usize) -> i32 {
   i32::try_from(value).unwrap_or(i32::MAX)
}

#[derive(Clone, Copy)]
struct Layout {
   columns:  usize,
   rows:     usize,
   scale:    usize,
   origin_x: usize,
   origin_y: usize,
}

impl Layout {
   #[expect(
      clippy::integer_division,
      clippy::integer_division_remainder_used,
      reason = "grid dimensions, integer scale, and centering offsets are floored"
   )]
   fn for_mode(mode: usize, screen: &Screen) -> Option<Self> {
      let (columns, rows) = match mode {
         0 => (80_usize, 25_usize),
         1 => (80_usize, 50_usize),
         2 => (screen.width() / GLYPH_WIDTH, screen.height() / GLYPH_HEIGHT),
         _ => return None,
      };

      if columns == 0 || rows == 0 {
         return None;
      }

      let scale =
         (screen.width() / (columns * GLYPH_WIDTH)).min(screen.height() / (rows * GLYPH_HEIGHT));

      if scale == 0 {
         return None;
      }

      Some(Self {
         columns,
         rows,
         scale,
         origin_x: (screen.width() - columns * GLYPH_WIDTH * scale) / 2,
         origin_y: (screen.height() - rows * GLYPH_HEIGHT * scale) / 2,
      })
   }

   const fn cell_width(&self) -> usize {
      GLYPH_WIDTH * self.scale
   }

   const fn cell_height(&self) -> usize {
      GLYPH_HEIGHT * self.scale
   }
}

#[derive(Clone, Copy)]
struct Cell {
   glyph:     char,
   attribute: u8,
}

#[repr(C)]
pub struct Console {
   // Must be at offset 0 so `*mut SimpleTextOutputProtocol` can be cast back to
   // `*mut Self`.
   protocol:  SimpleTextOutputProtocol,
   mode:      SimpleTextOutputMode,
   screen:    Screen,
   layout:    Layout,
   attribute: u8,
   column:    usize,
   row:       usize,
   cells:     Vec<Cell>,
   dirty:     Vec<Option<(usize, usize)>>,
   pixels:    Vec<GraphicsOutputBltPixel>,
   scratch:   Vec<GraphicsOutputBltPixel>,
}

impl Console {
   pub fn leak(screen: Screen) -> Option<&'static mut Self> {
      let (mode, layout) = [DEFAULT_MODE, 0]
         .into_iter()
         .find_map(|candidate| Layout::for_mode(candidate, &screen).map(|fit| (candidate, fit)))?;

      let console = Box::leak(Box::new(Self {
         protocol: SimpleTextOutputProtocol {
            reset,
            output_string,
            test_string,
            query_mode,
            set_mode,
            set_attribute,
            clear_screen,
            set_cursor_position,
            enable_cursor,
            mode: ptr::null(),
         },
         mode: SimpleTextOutputMode {
            max_mode:       mirror(MODE_COUNT),
            mode:           mirror(mode),
            attribute:      i32::from(DEFAULT_ATTRIBUTE),
            cursor_column:  0,
            cursor_row:     0,
            cursor_visible: Boolean::FALSE,
         },
         screen,
         layout,
         attribute: DEFAULT_ATTRIBUTE,
         column: 0,
         row: 0,
         cells: Vec::new(),
         dirty: Vec::new(),
         pixels: Vec::new(),
         scratch: Vec::new(),
      }));

      console.protocol.mode = &raw const console.mode;
      (console.apply(mode, layout) == Status::SUCCESS).then_some(console)
   }

   pub const fn protocol(&mut self) -> *mut SimpleTextOutputProtocol {
      &raw mut self.protocol
   }

   const unsafe fn from_protocol<'console>(
      this: *const SimpleTextOutputProtocol,
   ) -> Option<&'console mut Self> {
      // SAFETY: `Console` is `#[repr(C)]` with `protocol` at offset 0, and
      // `this` points to a leaked `Console`.
      unsafe { this.cast::<Self>().cast_mut().as_mut() }
   }

   fn set_attribute(&mut self, attribute: u8) {
      self.attribute = attribute;
      self.mode.attribute = i32::from(attribute);
   }

   fn move_cursor(&mut self, column: usize, row: usize) {
      self.column = column;
      self.row = row;
      self.mode.cursor_column = mirror(column);
      self.mode.cursor_row = mirror(row);
   }

   const fn blank(&self) -> Cell {
      Cell {
         glyph:     ' ',
         attribute: self.attribute,
      }
   }

   fn apply(&mut self, mode: usize, layout: Layout) -> Status {
      self.layout = layout;
      self.mode.mode = mirror(mode);
      self.cells = vec![self.blank(); layout.columns * layout.rows];
      self.dirty = vec![None; layout.rows];
      self.clear()
   }

   fn clear(&mut self) -> Status {
      let blank = self.blank();
      self.cells.fill(blank);
      self.dirty.fill(None);
      self.move_cursor(0, 0);
      self
         .screen
         .fill(self.screen.bounds(), background(blank.attribute))
   }

   fn mark(&mut self, column: usize, row: usize) {
      let span = self.dirty[row].map_or((column, column + 1), |(start, end)| {
         (start.min(column), end.max(column + 1))
      });
      self.dirty[row] = Some(span);
   }

   fn line_feed(&mut self) -> bool {
      if self.row + 1 < self.layout.rows {
         self.move_cursor(self.column, self.row + 1);
         return false;
      }

      let columns = self.layout.columns;
      let blank = self.blank();
      self.cells.copy_within(columns.., 0);
      let last = self.cells.len() - columns;
      self.cells[last..].fill(blank);
      true
   }

   fn put(&mut self, glyph: char) -> bool {
      let (column, row) = (self.column, self.row);
      self.cells[row * self.layout.columns + column] = Cell {
         glyph,
         attribute: self.attribute,
      };
      self.mark(column, row);

      if column + 1 < self.layout.columns {
         self.move_cursor(column + 1, row);
         return false;
      }

      self.move_cursor(0, row);
      self.line_feed()
   }

   fn output(&mut self, units: &[u16]) -> Status {
      let mut scrolled = false;
      let mut unknown = false;

      for &unit in units {
         match unit {
            0x0A => scrolled |= self.line_feed(),
            0x0D => self.move_cursor(0, self.row),
            0x08 => self.move_cursor(self.column.saturating_sub(1), self.row),
            0x00..=0x1F => {},
            _ => {
               let glyph = char::from_u32(u32::from(unit)).filter(|&known| raster(known).is_some());
               unknown |= glyph.is_none();
               scrolled |= self.put(glyph.unwrap_or('\u{FFFD}'));
            },
         }
      }

      let drawn = if scrolled {
         self.dirty.fill(None);
         self.draw_all()
      } else {
         self.draw_dirty()
      };

      if drawn != Status::SUCCESS {
         Status::DEVICE_ERROR
      } else if unknown {
         Status::WARN_UNKNOWN_GLYPH
      } else {
         Status::SUCCESS
      }
   }

   fn draw_all(&mut self) -> Status {
      for row in 0..self.layout.rows {
         let status = self.draw_run(row, 0, self.layout.columns);
         if status != Status::SUCCESS {
            return status;
         }
      }

      Status::SUCCESS
   }

   fn draw_dirty(&mut self) -> Status {
      for row in 0..self.layout.rows {
         if let Some((start, end)) = self.dirty[row].take() {
            let status = self.draw_run(row, start, end);
            if status != Status::SUCCESS {
               return status;
            }
         }
      }

      Status::SUCCESS
   }

   fn draw_run(&mut self, row: usize, start: usize, end: usize) -> Status {
      let layout = self.layout;
      let cell_width = layout.cell_width();
      let cell_height = layout.cell_height();
      let width = (end - start) * cell_width;
      let first = row * layout.columns;

      self.pixels.clear();
      self
         .pixels
         .resize(width * cell_height, GraphicsOutputBltPixel::default());

      for (index, cell) in self.cells[first + start..first + end].iter().enumerate() {
         let fore = foreground(cell.attribute);
         let back = background(cell.attribute);
         let bitmap = raster(cell.glyph);

         for glyph_row in 0..GLYPH_HEIGHT {
            for repeat in 0..layout.scale {
               let y = glyph_row * layout.scale + repeat;
               let line = &mut self.pixels[y * width + index * cell_width..][..cell_width];

               for (glyph_column, block) in line.chunks_exact_mut(layout.scale).enumerate() {
                  let intensity = bitmap
                     .as_ref()
                     .and_then(|rasterized| rasterized.pixel(glyph_column, glyph_row))
                     .unwrap_or(0);
                  block.fill(blend(back, fore, intensity));
               }
            }
         }
      }

      let rect = Rect {
         x: layout.origin_x + start * cell_width,
         y: layout.origin_y + row * cell_height,
         width,
         height: cell_height,
      };

      self.screen.draw(rect, &self.pixels, &mut self.scratch)
   }
}

unsafe fn units<'text>(string: *const Char16) -> &'text [u16] {
   // SAFETY: the caller guarantees `string` is a valid NUL-terminated UCS-2
   // string.
   unsafe { CStr16::from_ptr(string.cast()) }.to_u16_slice()
}

unsafe extern "efiapi" fn reset(this: *mut SimpleTextOutputProtocol, _extended: Boolean) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   console.set_attribute(DEFAULT_ATTRIBUTE);
   console.clear()
}

unsafe extern "efiapi" fn output_string(
   this: *mut SimpleTextOutputProtocol,
   string: *const Char16,
) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   if string.is_null() {
      return Status::INVALID_PARAMETER;
   }

   // SAFETY: UEFI requires `string` to be NUL-terminated.
   console.output(unsafe { units(string) })
}

unsafe extern "efiapi" fn test_string(
   _this: *mut SimpleTextOutputProtocol,
   string: *const Char16,
) -> Status {
   if string.is_null() {
      return Status::INVALID_PARAMETER;
   }

   // SAFETY: UEFI requires `string` to be NUL-terminated.
   let printable = unsafe { units(string) }
      .iter()
      .all(|&unit| unit < 0x20 || char::from_u32(u32::from(unit)).and_then(raster).is_some());

   if printable {
      Status::SUCCESS
   } else {
      Status::UNSUPPORTED
   }
}

unsafe extern "efiapi" fn query_mode(
   this: *const SimpleTextOutputProtocol,
   mode: usize,
   columns: *mut usize,
   rows: *mut usize,
) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   let Some(layout) = Layout::for_mode(mode, &console.screen) else {
      return Status::UNSUPPORTED;
   };

   // SAFETY: `columns` is an out-pointer from the UEFI caller.
   if let Some(column_slot) = unsafe { columns.as_mut() } {
      *column_slot = layout.columns;
   }

   // SAFETY: `rows` is an out-pointer from the UEFI caller.
   if let Some(row_slot) = unsafe { rows.as_mut() } {
      *row_slot = layout.rows;
   }

   Status::SUCCESS
}

unsafe extern "efiapi" fn set_mode(this: *mut SimpleTextOutputProtocol, mode: usize) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   Layout::for_mode(mode, &console.screen)
      .map_or(Status::UNSUPPORTED, |layout| console.apply(mode, layout))
}

unsafe extern "efiapi" fn set_attribute(
   this: *mut SimpleTextOutputProtocol,
   attribute: usize,
) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   match u8::try_from(attribute) {
      Ok(valid @ 0..=0x7F) => {
         console.set_attribute(valid);
         Status::SUCCESS
      },
      _ => Status::UNSUPPORTED,
   }
}

unsafe extern "efiapi" fn clear_screen(this: *mut SimpleTextOutputProtocol) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   console.clear()
}

unsafe extern "efiapi" fn set_cursor_position(
   this: *mut SimpleTextOutputProtocol,
   column: usize,
   row: usize,
) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   if column >= console.layout.columns || row >= console.layout.rows {
      return Status::UNSUPPORTED;
   }

   console.move_cursor(column, row);
   Status::SUCCESS
}

const unsafe extern "efiapi" fn enable_cursor(
   this: *mut SimpleTextOutputProtocol,
   visible: Boolean,
) -> Status {
   // SAFETY: `this` is the protocol pointer installed by this driver.
   let Some(console) = (unsafe { Console::from_protocol(this) }) else {
      return Status::INVALID_PARAMETER;
   };

   console.mode.cursor_visible = visible;
   Status::SUCCESS
}
