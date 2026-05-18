use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pixels::{Pixels, SurfaceTexture};
use rgb_core::{DebugInfo, GameBoy, JoypadState, StepResult, SCREEN_HEIGHT, SCREEN_WIDTH};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Scale factor: the 160×144 DMG screen is displayed at 3× (480×432).
const SCALE: u32 = 3;

/// Target frame duration for ~59.7 fps (70224 T-cycles / 4_194_304 Hz ≈ 16.742 ms).
const FRAME_DURATION: Duration = Duration::from_nanos(16_742_706);

// ---------------------------------------------------------------------------

/// Holds the winit window and pixels surface together so their lifetimes match.
struct RenderState {
    window: Arc<Window>,
    pixels: Pixels<'static>,
}

/// Main application state.
struct App {
    game_boy: GameBoy,
    render: Option<RenderState>,
    joypad: JoypadState,
    last_frame: Instant,
    /// Path to the `.sav` file for battery-backed cartridges, or `None`.
    save_path: Option<PathBuf>,
    /// Whether the F1 debug overlay is active.
    debug_overlay: bool,
    /// Whether the F2 VRAM tile viewer is active.
    tile_viewer: bool,
}

impl App {
    fn new(game_boy: GameBoy, save_path: Option<PathBuf>) -> Self {
        Self {
            game_boy,
            render: None,
            joypad: JoypadState::default(),
            last_frame: Instant::now(),
            save_path,
            debug_overlay: false,
            tile_viewer: false,
        }
    }

    /// Print the debug overlay to stdout.
    fn print_debug_overlay(info: &DebugInfo) {
        println!("=== DEBUG OVERLAY ===");
        println!("ROM: {}", if info.rom_title.is_empty() { "<unknown>" } else { &info.rom_title });
        println!("Cycles: {}", info.total_cycles);
        println!("--- CPU Registers ---");
        println!(" A={:#04X}  F={:#04X}  AF={:#06X}", info.a, info.f, (info.a as u16) << 8 | info.f as u16);
        println!(" B={:#04X}  C={:#04X}  BC={:#06X}", info.b, info.c, (info.b as u16) << 8 | info.c as u16);
        println!(" D={:#04X}  E={:#04X}  DE={:#06X}", info.d, info.e, (info.d as u16) << 8 | info.e as u16);
        println!(" H={:#04X}  L={:#04X}  HL={:#06X}", info.h, info.l, (info.h as u16) << 8 | info.l as u16);
        println!(" SP={:#06X}  PC={:#06X}", info.sp, info.pc);
        println!("--- Flags ---");
        println!(" Z={}  N={}  H={}  C={}  IME={}", info.flag_z as u8, info.flag_n as u8, info.flag_h as u8, info.flag_c as u8, info.ime as u8);
        println!("--- PPU ---");
        println!(" Mode={}  LY={}", info.ppu_mode, info.ly);
        println!("=====================");
    }

    /// Write a 128×192 PPM tile viewer image to `/tmp/rgb_tiles.ppm`.
    ///
    /// Layout: 16 tiles wide × 24 tiles tall, each tile 8×8 pixels.
    /// Palette applied from the BGP register.
    fn write_tile_viewer(vram: &[u8], bgp: u8) {
        // DMG grayscale palette: index → (R, G, B)
        const SHADES: [(u8, u8, u8); 4] = [
            (255, 255, 255), // 0 = white
            (170, 170, 170), // 1 = light gray
            (85, 85, 85),    // 2 = dark gray
            (0, 0, 0),       // 3 = black
        ];

        let w: usize = 16 * 8; // 128
        let h: usize = 24 * 8; // 192
        let mut pixels = vec![(255u8, 255u8, 255u8); w * h];

        for tile_idx in 0..384usize {
            let tile_col = tile_idx % 16;
            let tile_row = tile_idx / 16;
            let base = tile_idx * 16;

            for row in 0..8usize {
                let lo = vram[base + row * 2];
                let hi = vram[base + row * 2 + 1];
                for col in 0..8usize {
                    let color_idx = ((hi >> (7 - col)) & 1) << 1 | ((lo >> (7 - col)) & 1);
                    let shade_idx = (bgp >> (color_idx * 2)) & 0x03;
                    let px = tile_col * 8 + col;
                    let py = tile_row * 8 + row;
                    pixels[py * w + px] = SHADES[shade_idx as usize];
                }
            }
        }

        // Write PPM P6 format.
        let path = "/tmp/rgb_tiles.ppm";
        let header = format!("P6\n{} {}\n255\n", w, h);
        let mut buf = Vec::with_capacity(header.len() + w * h * 3);
        buf.extend_from_slice(header.as_bytes());
        for (r, g, b) in &pixels {
            buf.push(*r);
            buf.push(*g);
            buf.push(*b);
        }
        if let Err(e) = std::fs::write(path, &buf) {
            eprintln!("[desktop] tile viewer write error: {e}");
        }
    }
    fn persist_save(&self) {
        if let (Some(ram), Some(path)) = (self.game_boy.battery_ram(), &self.save_path) {
            match std::fs::write(path, &ram) {
                Ok(()) => eprintln!("[desktop] save written to {}", path.display()),
                Err(e) => eprintln!("[desktop] failed to write save '{}': {e}", path.display()),
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.persist_save();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("rgb-emulator")
            .with_inner_size(LogicalSize::new(SCREEN_WIDTH * SCALE, SCREEN_HEIGHT * SCALE))
            .with_resizable(false);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let inner = window.inner_size();
        let surface_texture =
            SurfaceTexture::new(inner.width, inner.height, Arc::clone(&window));
        let pixels = Pixels::new(SCREEN_WIDTH, SCREEN_HEIGHT, surface_texture)
            .expect("failed to create pixel buffer");

        self.render = Some(RenderState { window, pixels });
        self.last_frame = Instant::now();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(key),
                        state,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                match key {
                    KeyCode::Escape => event_loop.exit(),
                    KeyCode::F1 if pressed => {
                        self.debug_overlay = !self.debug_overlay;
                        Self::print_debug_overlay(&self.game_boy.debug_info());
                    }
                    KeyCode::F2 if pressed => {
                        self.tile_viewer = !self.tile_viewer;
                        if self.tile_viewer {
                            println!("Tile viewer: ON — writing to /tmp/rgb_tiles.ppm");
                        } else {
                            println!("Tile viewer: OFF");
                        }
                    }
                    KeyCode::ArrowUp => self.joypad.up = pressed,
                    KeyCode::ArrowDown => self.joypad.down = pressed,
                    KeyCode::ArrowLeft => self.joypad.left = pressed,
                    KeyCode::ArrowRight => self.joypad.right = pressed,
                    KeyCode::KeyZ => self.joypad.a = pressed,
                    KeyCode::KeyX => self.joypad.b = pressed,
                    KeyCode::Enter => self.joypad.start = pressed,
                    KeyCode::ShiftRight => self.joypad.select = pressed,
                    _ => {}
                }
            }

            WindowEvent::RedrawRequested => {
                if let Some(render) = &mut self.render {
                    let frame = render.pixels.frame_mut();
                    frame.copy_from_slice(self.game_boy.framebuffer());

                    if let Err(e) = render.pixels.render() {
                        eprintln!("[desktop] render error: {e}");
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Run the emulator until a full frame has been produced.
        loop {
            match self.game_boy.step(&self.joypad) {
                Ok(StepResult::FrameComplete) => break,
                Ok(StepResult::Continue) => {}
                Err(e) => panic!("[desktop] emulation error: {e}"),
            }
        }

        // Print any serial output bytes produced this frame (blargg test ROMs
        // use the serial port to report pass/fail before the PPU is working).
        if !self.game_boy.serial_output.is_empty() {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for &byte in &self.game_boy.serial_output {
                let _ = out.write_all(&[byte]);
            }
            let _ = out.flush();
            self.game_boy.serial_output.clear();
        }

        // Throttle to target frame rate.
        let target = self.last_frame + FRAME_DURATION;
        let now = Instant::now();
        if now < target {
            std::thread::sleep(target - now);
        }
        self.last_frame = Instant::now();

        // Request redraw to blit the framebuffer.
        if let Some(render) = &self.render {
            render.window.request_redraw();
        }

        // When the tile viewer is enabled, write the PPM each frame.
        if self.tile_viewer {
            Self::write_tile_viewer(self.game_boy.vram_tiles(), self.game_boy.bgp());
        }
    }
}

// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <rom> [boot_rom]", args[0]);
        std::process::exit(1);
    }

    let rom_path = &args[1];
    let boot_rom_path = args.get(2);

    let rom = std::fs::read(rom_path).unwrap_or_else(|e| {
        eprintln!("Failed to read ROM '{}': {e}", rom_path);
        std::process::exit(1);
    });

    let boot_rom = boot_rom_path.map(|path| {
        std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read boot ROM '{}': {e}", path);
            std::process::exit(1);
        })
    });

    let mut game_boy = GameBoy::new(rom, boot_rom);

    // Derive the .sav path from the ROM path (e.g. "game.gb" → "game.sav").
    let save_path = {
        let mut p = PathBuf::from(rom_path);
        p.set_extension("sav");
        p
    };

    // Load an existing save file if one is present.
    if save_path.exists() {
        match std::fs::read(&save_path) {
            Ok(data) => {
                game_boy.load_battery_ram(&data);
                eprintln!("[desktop] save loaded from {}", save_path.display());
            }
            Err(e) => eprintln!(
                "[desktop] failed to read save '{}': {e}",
                save_path.display()
            ),
        }
    }

    // Only track the save path when the cartridge actually has a battery.
    let save_path = game_boy.battery_ram().map(|_| save_path);

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(game_boy, save_path);
    event_loop.run_app(&mut app).expect("event loop error");
}
