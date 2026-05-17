//! Headless test runner for blargg test ROMs.
//!
//! Usage:
//!   ROM_PATH=path/to/test.gb cargo run --bin blargg-test
//!   TIMEOUT_CYCLES=100000000 ROM_PATH=... cargo run --bin blargg-test
//!
//! Exit codes:
//!   0 — "Passed" found in serial output
//!   1 — "Failed" found in serial output, or timeout reached

use std::env;
use rgb_core::{GameBoy, JoypadState};

fn main() {
    let rom_path = env::var("ROM_PATH")
        .expect("ROM_PATH environment variable must be set");
    let timeout: u64 = env::var("TIMEOUT_CYCLES")
        .unwrap_or_else(|_| "100000000".to_string())
        .parse()
        .expect("TIMEOUT_CYCLES must be a number");

    let rom = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("Failed to read ROM '{}': {}", rom_path, e));

    let mut gb = GameBoy::new(rom, None);
    let joypad = JoypadState::default();
    let mut total_cycles: u64 = 0;
    let mut output = String::new();

    loop {
        let result = gb.step(&joypad).expect("Emulation error");

        // Drain serial output
        if !gb.serial_output.is_empty() {
            let bytes: Vec<u8> = gb.serial_output.drain(..).collect();
            let chunk = String::from_utf8_lossy(&bytes).to_string();
            print!("{}", chunk);
            output.push_str(&chunk);

            if output.contains("Passed") {
                println!("\n[PASS] {}", rom_path);
                std::process::exit(0);
            }
            if output.contains("Failed") {
                println!("\n[FAIL] {}", rom_path);
                std::process::exit(1);
            }
        }

        total_cycles += 4; // approximate: step() advances at least 4 T-cycles
        if total_cycles >= timeout {
            eprintln!("[TIMEOUT] {} after {} cycles", rom_path, total_cycles);
            eprintln!("Serial output so far: {:?}", output);
            std::process::exit(1);
        }

        let _ = result;
    }
}
