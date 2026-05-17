# rgb-emulator — Product Requirements Document

## Overview

A Game Boy (DMG) emulator written in Rust. Third attempt at this project — the first in Rust (abandoned due to learning Rust and emulation simultaneously), the second in TypeScript (stalled on PPU implementation). This attempt targets correctness over speed-to-first-boot, with a clean architecture designed to avoid the pitfalls of previous attempts.

---

## Goals

- Pass `blargg/cpu_instrs` (all 11 subtests)
- Pass `blargg/instr_timing`
- Pass `dmg-acid2`
- Run the DMG boot ROM correctly (scrolling logo, chime)
- Run commercial games: Tetris, Super Mario Land, Zelda: Link's Awakening, Kirby's Dream Land
- Clean separation between emulation core and frontend to allow future WASM target

## Non-Goals (for now)

- Game Boy Color (CGB) support
- Game Boy Pocket / Super Game Boy variants
- Link cable / multiplayer
- Full save state support (architecture supports it, implementation deferred)
- Full interactive debugger (step mode + register dump is sufficient for Phase 1-3)

---

## Naming Conventions

Acronyms are written in full uppercase — `CPU`, `PPU`, `APU`, `MBC`, `OAM`, `DMA`, `FIFO`. The clippy `upper_case_acronyms` lint is suppressed project-wide in `Cargo.toml`. Do not write `Cpu`, `Ppu`, `Mbc`, etc.

---

## Architecture

### Crate Structure

Cargo workspace with two crates:

```
rgb-emulator/
  Cargo.toml              (workspace root)
  crates/
    core/                 (CPU, PPU, APU, Bus — no I/O, no rendering deps)
    desktop/              (winit, pixels, cpal — ties core to a window)
  docs/
```

`core` is fully platform-agnostic. `desktop` depends on `core`. A future `wasm` crate could depend on `core` independently.

### God Struct

`GameBoy` owns all components. Components receive `&mut Bus` per step call rather than owning or referencing it. The borrow checker is satisfied because `cpu`, `ppu`, and `bus` are distinct fields.

```rust
struct GameBoy {
    cpu: Cpu,
    ppu: Ppu,
    timer: Timer,
    bus: Bus,
    serial_output: Vec<u8>,
}

impl GameBoy {
    pub fn step(&mut self, input: &JoypadState) -> Result<StepResult, EmulationError> { ... }
}
```

### Memory Bus

- `Bus` owns: cartridge (`Box<dyn Mbc>`), VRAM, WRAM, OAM, HRAM, IO registers
- Address dispatch via `match` on `u16` ranges
- Invalid/unmapped addresses: `panic!` internally during development
- `GameBoy::step()` returns `Result` — the only graceful error boundary
- Architecture kept free of non-serializable types for future save state support

---

## Components

### CPU

- Sharp SM83 (DMG variant)
- Instruction-accurate execution with correct cycle counts
- Instruction decoding: `match` on raw `u8` opcode
- CB-prefix instructions: nested `match` on the following byte
- Interrupts checked after every instruction
- HALT bug implemented from day one
- Registers: `Cpu` struct with named `u8` fields (`a`, `b`, `c`, `d`, `e`, `h`, `l`, `f`) and `u16` fields (`sp`, `pc`)
- Flag register helpers: `zero()`, `subtract()`, `half_carry()`, `carry()`

### PPU

- FIFO pixel pipeline — cycle-accurate dot rendering
- Background fetcher + sprite fetcher + two pixel FIFOs (background, sprite)
- Mode timing:
  - Mode 2 (OAM Scan): 80 T-cycles
  - Mode 3 (Drawing): 172–289 T-cycles (variable — SCX fine scroll + sprite count)
  - Mode 0 (HBlank): remaining cycles to fill 456
  - Mode 1 (VBlank): lines 144–153, 456 T-cycles each
- STAT register fully implemented: mode-change interrupts, LY coincidence interrupt
- OAM DMA: instant implementation initially (`0xFF46` write triggers immediate 160-byte copy), cycle-accurate deferred
- Output: 160x144 RGBA framebuffer exposed on `GameBoy`, read by `desktop` each VBlank

### Timer

- Single internal 16-bit counter ticking every T-cycle
- `DIV` (`0xFF04`) is the upper byte of this counter — writes reset the full counter to 0
- `TIMA` (`0xFF05`) driven by a specific bit of the internal counter selected by `TAC`
- 1 T-cycle delay between `TIMA` overflow and `TMA` reload, during which `TIMA` reads `0x00`
- Timer overflow requests interrupt bit 2 in `IF`

### APU

- Stubbed in Phase 1: reads return open-bus values, writes are silently ignored
- Full implementation in Phase 5
- 4 channels: CH1 (pulse + sweep), CH2 (pulse), CH3 (wave), CH4 (noise)
- Frame sequencer driven off the same internal `DIV` counter as the timer
- `cpal` ring buffer output in `desktop`

### Cartridge / MBC

- MBC type detected from header byte `0x0147`
- Dispatched via `Box<dyn Mbc>` trait object
- Phase 1: ROM-only (MBC0) — no banking
- Phase 4: MBC1 — ROM/RAM banking
- Battery saves (persist cartridge RAM to disk) implemented alongside MBC1

### Joypad

- `JoypadState` struct (8 booleans or `u8` bitfield) passed into `GameBoy::step()`
- `desktop` maps `winit` keycodes to `JoypadState`
- `core` translates to `0xFF00` register format and handles joypad interrupt

### Serial Port

- Stub implementation: when `SC` (`0xFF02`) is written with `0x81`, byte in `SB` (`0xFF01`) is appended to `GameBoy::serial_output`
- `desktop` prints `serial_output` to stdout
- Used by `blargg` test ROMs to report pass/fail before PPU is working

### Boot ROM

- Optional: if a `dmg_boot.bin` path is provided, it is loaded and executed from `0x0000`
- Default: all hardware registers initialised to documented post-boot state, PC set to `0x0100`
- Boot ROM unmaps itself by writing to `0xFF50`

---

## Frontend (`desktop`)

| Concern | Library |
|---|---|
| Windowing + input | `winit` |
| Pixel buffer rendering | `pixels` (wgpu-backed) |
| Audio output | `cpal` |

- Fixed 160x144 internal resolution, scaled to window with nearest-neighbor filtering
- Target 59.7 fps (DMG frame rate)
- Joypad default mapping: Arrow keys → D-pad, Z → A, X → B, Enter → Start, Shift → Select
- Debug overlay (toggled by keypress): register dump, PPU mode/LY, cycle count
- VRAM tile viewer (separate debug window): raw tile data + OAM

---

## Error Handling

- Internal: `panic!` / `unreachable!()` for invalid opcodes, bad addresses, and impossible states — loud and fatal during development
- External boundary: `GameBoy::step()` returns `Result<StepResult, EmulationError>` — `desktop` catches this and halts cleanly with an error message

---

## Testing Strategy

### Test ROM Ladder

| Phase | Target | Passing Criteria |
|---|---|---|
| 2 | `blargg/cpu_instrs` | All 11 subtests report "Passed" via serial output |
| 2 | `blargg/instr_timing` | Reports "Passed" via serial output |
| 3 | `dmg-acid2` | Pixel-perfect match to reference image |
| 4 | Boot ROM | Nintendo logo scrolls, chime plays, jumps to `0x0100` |

### Headless Test Harness

- `core` exposes no windowing or audio dependencies
- Test binary in `core` loads a ROM, runs `GameBoy::step()` in a loop, reads `serial_output`
- CI runs `blargg` tests headlessly on every push

---

## Development Milestones

### Phase 1 — Scaffold
- Cargo workspace initialised
- `winit` + `pixels` window opens, game loop running
- Bus with hardcoded post-boot register state
- Optional boot ROM loading
- ROM-only cartridge loading (header parsing, ROM mapped to bus)
- Serial stub writing to stdout

### Phase 2 — CPU
- Full SM83 instruction set
- Interrupts + HALT bug
- Timer (single internal counter)
- **Exit criteria:** `blargg/cpu_instrs` all 11 subtests pass, `blargg/instr_timing` passes

### Phase 3 — PPU
- FIFO pixel pipeline (background, window, sprites)
- Instant OAM DMA
- STAT interrupts, LY coincidence
- Framebuffer output in `desktop`
- **Exit criteria:** `dmg-acid2` passes, boot ROM scrolls logo, Tetris is playable

### Phase 4 — Polish
- MBC1 cartridge support + battery saves
- Joypad input wired to `winit` keycodes
- Debug overlay + VRAM tile viewer
- **Exit criteria:** Super Mario Land, Zelda: Link's Awakening, Kirby's Dream Land run correctly

### Phase 5 — Audio
- All 4 APU channels implemented
- `cpal` ring buffer output
- **Exit criteria:** Tetris music plays correctly, no major audio artifacts

---

## Future Considerations

- WASM target via `wasm` crate depending on `core`
- Game Boy Color (CGB) support
- Cycle-accurate OAM DMA
- Full save states (`serde` on all `core` structs)
- Full interactive debugger (breakpoints, memory viewer, disassembler)
- MBC2, MBC3 (RTC), MBC5
