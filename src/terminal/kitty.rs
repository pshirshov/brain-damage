use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::io::{self, Write};

const CHUNK_SIZE: usize = 4096;

/// Kitty graphics protocol implementation
pub struct KittyGraphics {
    image_id: u32,
    last_width: u32,
    last_height: u32,
}

impl KittyGraphics {
    pub fn new() -> Self {
        Self {
            image_id: 1,
            last_width: 0,
            last_height: 0,
        }
    }

    /// Clear the screen and prepare for graphics
    pub fn setup_terminal(&self) -> Result<()> {
        let mut stdout = io::stdout().lock();

        // Enter alternate screen buffer
        write!(stdout, "\x1b[?1049h")?;
        // Hide cursor
        write!(stdout, "\x1b[?25l")?;
        // Clear screen
        write!(stdout, "\x1b[2J")?;
        // Move cursor to top-left
        write!(stdout, "\x1b[H")?;

        stdout.flush()?;
        Ok(())
    }

    /// Restore terminal state
    pub fn restore_terminal(&self) -> Result<()> {
        let mut stdout = io::stdout().lock();

        // Clear any displayed images
        write!(stdout, "\x1b_Ga=d;\x1b\\")?;
        // Show cursor
        write!(stdout, "\x1b[?25h")?;
        // Leave alternate screen buffer
        write!(stdout, "\x1b[?1049l")?;

        stdout.flush()?;
        Ok(())
    }

    /// Display RGBA image data at the current cursor position
    pub fn display_frame(&mut self, width: u32, height: u32, rgba_data: &[u8]) -> Result<()> {
        tracing::trace!("Displaying frame: {}x{}, {} bytes", width, height, rgba_data.len());

        // Scale down large images to fit terminal better
        let (scaled_data, scaled_width, scaled_height) = if width > 1920 || height > 1080 {
            let scale = f32::min(1920.0 / width as f32, 1080.0 / height as f32);
            let new_width = (width as f32 * scale) as u32;
            let new_height = (height as f32 * scale) as u32;
            tracing::trace!("Scaling {}x{} -> {}x{}", width, height, new_width, new_height);
            (scale_image(rgba_data, width, height, new_width, new_height), new_width, new_height)
        } else {
            (rgba_data.to_vec(), width, height)
        };

        let mut stdout = io::stdout().lock();

        // Delete previous image if dimensions changed
        if self.last_width != scaled_width || self.last_height != scaled_height {
            write!(stdout, "\x1b_Ga=d;\x1b\\")?;
            self.last_width = scaled_width;
            self.last_height = scaled_height;
        }

        // Move cursor to top-left
        write!(stdout, "\x1b[H")?;

        // Send uncompressed for now (compression has display issues with Kitty)
        let encoded = BASE64.encode(&scaled_data);
        let compression_flag = "";

        // Send image in chunks
        let chunks: Vec<&str> = encoded
            .as_bytes()
            .chunks(CHUNK_SIZE)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();

        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let is_first = i == 0;

            if is_first {
                // First chunk: include all parameters
                // a=T: transmit and display
                // f=32: RGBA format
                // s,v: source width, height
                // o=z: zstd compression (optional)
                // m=0/1: more chunks flag
                // i: image id for replacement
                // q=2: suppress responses
                write!(
                    stdout,
                    "\x1b_Ga=T,f=32,s={},v={}{},m={},i={},q=2;{}\x1b\\",
                    scaled_width,
                    scaled_height,
                    compression_flag,
                    if is_last { 0 } else { 1 },
                    self.image_id,
                    chunk
                )?;
            } else {
                // Continuation chunk
                write!(
                    stdout,
                    "\x1b_Gm={};{}\x1b\\",
                    if is_last { 0 } else { 1 },
                    chunk
                )?;
            }
        }

        stdout.flush()?;

        // Cycle image ID for next frame (allows replacement)
        self.image_id = if self.image_id >= 1000 { 1 } else { self.image_id + 1 };

        Ok(())
    }

    /// Get terminal size in pixels (if available)
    pub fn query_terminal_size_pixels() -> Result<(u32, u32)> {
        // Try to use TIOCGWINSZ to get pixel dimensions
        use std::os::unix::io::AsRawFd;

        let stdout = io::stdout();
        let fd = stdout.as_raw_fd();

        let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut winsize) };

        if result == 0 && winsize.ws_xpixel > 0 && winsize.ws_ypixel > 0 {
            Ok((winsize.ws_xpixel as u32, winsize.ws_ypixel as u32))
        } else {
            // Fallback: estimate based on cell size
            let (cols, rows) = crossterm::terminal::size()?;
            // Assume typical cell size of 10x20 pixels
            Ok((cols as u32 * 10, rows as u32 * 20))
        }
    }

    /// Get terminal size in characters
    pub fn query_terminal_size_chars() -> Result<(u16, u16)> {
        Ok(crossterm::terminal::size()?)
    }
}

impl Default for KittyGraphics {
    fn default() -> Self {
        Self::new()
    }
}

/// Scale RGBA image data using bilinear interpolation
fn scale_image(data: &[u8], src_width: u32, src_height: u32, dst_width: u32, dst_height: u32) -> Vec<u8> {
    let mut result = vec![0u8; (dst_width * dst_height * 4) as usize];

    let x_ratio = src_width as f32 / dst_width as f32;
    let y_ratio = src_height as f32 / dst_height as f32;

    for dst_y in 0..dst_height {
        for dst_x in 0..dst_width {
            let src_x = dst_x as f32 * x_ratio;
            let src_y = dst_y as f32 * y_ratio;

            let x0 = src_x.floor() as u32;
            let y0 = src_y.floor() as u32;
            let x1 = (x0 + 1).min(src_width - 1);
            let y1 = (y0 + 1).min(src_height - 1);

            let x_frac = src_x - x0 as f32;
            let y_frac = src_y - y0 as f32;

            let dst_idx = ((dst_y * dst_width + dst_x) * 4) as usize;

            for c in 0..4 {
                let p00 = data[((y0 * src_width + x0) * 4) as usize + c] as f32;
                let p10 = data[((y0 * src_width + x1) * 4) as usize + c] as f32;
                let p01 = data[((y1 * src_width + x0) * 4) as usize + c] as f32;
                let p11 = data[((y1 * src_width + x1) * 4) as usize + c] as f32;

                let top = p00 * (1.0 - x_frac) + p10 * x_frac;
                let bottom = p01 * (1.0 - x_frac) + p11 * x_frac;
                let value = top * (1.0 - y_frac) + bottom * y_frac;

                result[dst_idx + c] = value as u8;
            }
        }
    }

    result
}
