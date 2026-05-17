# rgb-emulator — Project Context

A Game Boy (DMG-only) emulator written in Rust. Full design decisions are in `docs/PRD.md`.

---

## Crate Structure

```
rgb-emulator/
  crates/
    core/      # Emulation logic — no platform deps, no I/O
    desktop/   # winit + pixels + cpal frontend
```

`desktop` depends on `core`. `core` must remain platform-agnostic (future WASM target).

---

## Architecture: God Struct

`GameBoy` in `core` owns all components. Components receive `&mut Bus` per step — they do not own or store references to it.

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

The borrow checker is satisfied because `cpu`, `ppu`, `timer`, and `bus` are distinct fields — Rust allows simultaneous `&mut` borrows of distinct struct fields.

---

## Key Decisions

| Concern | Decision |
|---|---|
| Target hardware | DMG only (no CGB) |
| Accuracy | Scanline-accurate baseline, cycle-accurate FIFO PPU |
| CPU decoding | `match` on raw `u8` opcode; CB-prefix is a nested `match` |
| Memory bus | `match` on `u16` address ranges; unmapped = `panic!` |
| Error handling | `panic!`/`unreachable!()` internally; `Result` at `GameBoy::step()` |
| Interrupts | Checked after every instruction; HALT bug implemented from day one |
| Timer | Single internal 16-bit counter; `DIV` is the upper byte |
| PPU | FIFO pixel pipeline — background fetcher, sprite fetcher, two FIFOs |
| OAM DMA | Instant (non-cycle-accurate) — `// TODO: cycle-accurate DMA` |
| APU | Stubbed (reads return open-bus, writes ignored) until Phase 5 |
| Cartridge | `Box<dyn Mbc>` dispatch via header `0x0147`; ROM-only first, then MBC1 |
| Boot ROM | Optional; hardcoded post-boot state if not supplied |
| Joypad | `JoypadState` struct passed into `GameBoy::step()` |
| Serial | Stub: writes to `SB`/`SC` append to `GameBoy::serial_output` |
| Save states | Architecture kept clean for future `serde` derivation |

---

## Frontend (`desktop`)

- **Windowing + input:** `winit`
- **Pixel buffer:** `pixels` (wgpu-backed, 160x144 scaled to window)
- **Audio:** `cpal` with ring buffer (Phase 5)
- **Joypad mapping:** Arrow keys → D-pad, Z → A, X → B, Enter → Start, Right Shift → Select
- **Debug overlay:** F1 — register dump, PPU mode/LY, cycle count
- **VRAM tile viewer:** F2 — second window, 16x24 tile grid

---

## Development Phases

| Phase | Focus | Exit Criterion |
|---|---|---|
| 1 | Scaffold | Window opens, ROM loads, serial stub works |
| 2 | CPU | `blargg/cpu_instrs` + `instr_timing` pass |
| 3 | PPU | `dmg-acid2` passes, Tetris playable |
| 4 | Polish | MBC1, joypad, SML/Zelda/Kirby run |
| 5 | Audio | Tetris music plays correctly |

---

## Repository

- GitHub: https://github.com/tjpeden/rgb-emulator
- Issues are tracked per-phase using GitHub milestones
- Commit convention: `feat: <description> (closes #N)`
- Branch convention: `issue-N`
