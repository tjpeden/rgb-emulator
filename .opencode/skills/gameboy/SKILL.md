---
name: gameboy
description: Game Boy (DMG) hardware reference. Use when implementing or debugging any DMG hardware component — CPU, PPU, timer, APU, joypad, serial, or memory map. Load this skill when you need register addresses, timing values, or hardware behavior details.
---

# Game Boy DMG Hardware Reference

Quick reference for DMG hardware implementation. Authoritative source: https://gbdev.io/pandocs/

---

## Memory Map

| Range | Region | Notes |
|---|---|---|
| `0x0000–0x00FF` | Boot ROM | Unmapped after `0xFF50` write |
| `0x0000–0x3FFF` | ROM Bank 0 | Fixed |
| `0x4000–0x7FFF` | ROM Bank N | Switchable (MBC) |
| `0x8000–0x9FFF` | VRAM | 8 KiB |
| `0xA000–0xBFFF` | External RAM | MBC-controlled |
| `0xC000–0xDFFF` | WRAM | 8 KiB |
| `0xE000–0xFDFF` | Echo RAM | Mirrors `0xC000–0xDDFF` |
| `0xFE00–0xFE9F` | OAM | 40 sprites × 4 bytes |
| `0xFEA0–0xFEFF` | Unused | Returns `0xFF` on read |
| `0xFF00–0xFF7F` | IO Registers | See table below |
| `0xFF80–0xFFFE` | HRAM | 127 bytes |
| `0xFFFF` | IE Register | Interrupt Enable |

---

## IO Registers

| Address | Name | Description |
|---|---|---|
| `0xFF00` | JOYP | Joypad |
| `0xFF01` | SB | Serial transfer data |
| `0xFF02` | SC | Serial transfer control |
| `0xFF04` | DIV | Divider (upper byte of internal counter) |
| `0xFF05` | TIMA | Timer counter |
| `0xFF06` | TMA | Timer modulo |
| `0xFF07` | TAC | Timer control |
| `0xFF0F` | IF | Interrupt Flag |
| `0xFF10` | NR10 | CH1 sweep |
| `0xFF11` | NR11 | CH1 length/duty |
| `0xFF12` | NR12 | CH1 volume envelope |
| `0xFF13` | NR13 | CH1 frequency low |
| `0xFF14` | NR14 | CH1 frequency high / trigger |
| `0xFF16` | NR21 | CH2 length/duty |
| `0xFF17` | NR22 | CH2 volume envelope |
| `0xFF18` | NR23 | CH2 frequency low |
| `0xFF19` | NR24 | CH2 frequency high / trigger |
| `0xFF1A` | NR30 | CH3 enable |
| `0xFF1B` | NR31 | CH3 length |
| `0xFF1C` | NR32 | CH3 volume |
| `0xFF1D` | NR33 | CH3 frequency low |
| `0xFF1E` | NR34 | CH3 frequency high / trigger |
| `0xFF20` | NR41 | CH4 length |
| `0xFF21` | NR42 | CH4 volume envelope |
| `0xFF22` | NR43 | CH4 frequency / LFSR |
| `0xFF23` | NR44 | CH4 trigger |
| `0xFF24` | NR50 | Master volume / VIN panning |
| `0xFF25` | NR51 | Sound panning |
| `0xFF26` | NR52 | Sound enable |
| `0xFF30–0xFF3F` | Wave RAM | CH3 waveform data (32 × 4-bit samples) |
| `0xFF40` | LCDC | LCD control |
| `0xFF41` | STAT | LCD status |
| `0xFF42` | SCY | Background scroll Y |
| `0xFF43` | SCX | Background scroll X |
| `0xFF44` | LY | Current scanline (0–153) |
| `0xFF45` | LYC | LY compare |
| `0xFF46` | DMA | OAM DMA source (write `XX` → copy `XX00–XX9F` to OAM) |
| `0xFF47` | BGP | Background palette |
| `0xFF48` | OBP0 | Object palette 0 |
| `0xFF49` | OBP1 | Object palette 1 |
| `0xFF4A` | WY | Window Y position |
| `0xFF4B` | WX | Window X position (actual pixel = WX - 7) |
| `0xFF50` | BOOT | Write non-zero to unmap boot ROM |
| `0xFFFF` | IE | Interrupt Enable |

---

## CPU — SM83 Post-Boot Register State

| Register | Value |
|---|---|
| AF | `0x01B0` |
| BC | `0x0013` |
| DE | `0x00D8` |
| HL | `0x014D` |
| SP | `0xFFFE` |
| PC | `0x0100` |

Flags (F = `0xB0`): Z=1, N=0, H=1, C=1

---

## Interrupts

| Bit | Source | Vector |
|---|---|---|
| 0 | VBlank | `0x0040` |
| 1 | LCD STAT | `0x0048` |
| 2 | Timer | `0x0050` |
| 3 | Serial | `0x0058` |
| 4 | Joypad | `0x0060` |

- `IE` (`0xFFFF`): enables interrupt sources
- `IF` (`0xFF0F`): pending interrupt flags (set by hardware, cleared on dispatch)
- `IME`: master enable, set by `EI`/`RETI`, cleared by `DI` or interrupt dispatch
- Service: if `IME && (IE & IF) != 0` after instruction — push PC, clear IF bit, jump to vector (5 M-cycles)
- `EI` enables IME after the *next* instruction (1-cycle delay)
- **HALT bug:** `HALT` with `IME=0` and pending interrupt → next instruction executes twice

---

## Timer

- One internal 16-bit counter, increments every T-cycle
- `DIV` = upper byte (`counter >> 8`); any write resets full counter to 0
- `TIMA` clocked by a counter bit selected by `TAC` bits 1–0:

| TAC bits 1–0 | Counter bit | Frequency |
|---|---|---|
| `00` | bit 9 | 4096 Hz |
| `01` | bit 3 | 262144 Hz |
| `10` | bit 5 | 65536 Hz |
| `11` | bit 7 | 16384 Hz |

- `TIMA` only increments when `TAC` bit 2 is set
- On `TIMA` overflow: 1 T-cycle delay (TIMA = 0x00), then reload from `TMA`, request Timer interrupt
- Writing to `DIV` can inadvertently clock `TIMA` if the selected bit falls from 1→0

---

## PPU

### LCDC bits (`0xFF40`)

| Bit | Name | Effect when set |
|---|---|---|
| 7 | LCD enable | Display on (off = white screen, LY=0) |
| 6 | Window tile map | Use `0x9C00` (else `0x9800`) |
| 5 | Window enable | Show window layer |
| 4 | BG/Win tile data | Use `0x8000` unsigned (else `0x8800` signed) |
| 3 | BG tile map | Use `0x9C00` (else `0x9800`) |
| 2 | OBJ size | 8×16 (else 8×8) |
| 1 | OBJ enable | Show sprites |
| 0 | BG/Win enable | Show background and window |

### STAT bits (`0xFF41`)

| Bit | Name |
|---|---|
| 6 | LYC=LY interrupt enable |
| 5 | Mode 2 interrupt enable |
| 4 | Mode 1 interrupt enable |
| 3 | Mode 0 interrupt enable |
| 2 | LYC=LY coincidence flag (read-only) |
| 1–0 | PPU mode (read-only): 0=HBlank, 1=VBlank, 2=OAM, 3=Drawing |

### Scanline Timing (456 T-cycles per line)

| Mode | Duration |
|---|---|
| 2 — OAM Scan | 80 T-cycles (fixed) |
| 3 — Drawing | 172–289 T-cycles (variable: SCX fine scroll + sprites) |
| 0 — HBlank | 456 − mode2 − mode3 T-cycles |
| 1 — VBlank | Lines 144–153, 456 T-cycles each |

### FIFO Fetcher States

Background fetcher (one step per 2 T-cycles):
1. Fetch tile number from tile map
2. Fetch tile data low
3. Fetch tile data high
4. Push 8 pixels to background FIFO

### OAM Sprite Attributes (4 bytes each)

| Byte | Field |
|---|---|
| 0 | Y position (sprite top = Y − 16) |
| 1 | X position (sprite left = X − 8) |
| 2 | Tile index |
| 3 | Flags: bit7=BG priority, bit6=Y flip, bit5=X flip, bit4=palette (OBP0/OBP1) |

### Palettes

BGP/OBP0/OBP1: each byte holds 4 × 2-bit entries. Bits 1–0 = color 0, bits 3–2 = color 1, etc.
DMG shades: 0=white, 1=light grey, 2=dark grey, 3=black.

---

## Joypad (`0xFF00`)

Write to select group; read to get active-low button state:

| Bit 5 | Bit 4 | Bits 3–0 |
|---|---|---|
| 0 | 1 | Buttons: Start, Select, B, A |
| 1 | 0 | D-pad: Down, Up, Left, Right |

Bit = 0 means pressed. Request Joypad interrupt (IF bit 4) on any button press.

---

## Serial (`0xFF01`/`0xFF02`)

- `SB` (`0xFF01`): data byte to transfer
- `SC` (`0xFF02`): control — bit 7 = transfer start, bit 0 = internal clock
- On `SC` write of `0x81`: transfer byte in `SB`, append to `serial_output`
- Used by blargg test ROMs to report results

---

## Cartridge Header

| Address | Field |
|---|---|
| `0x0104–0x0133` | Nintendo logo (must match or boot ROM locks up) |
| `0x0134–0x0143` | Title (ASCII, padded with `0x00`) |
| `0x0147` | Cartridge type (MBC select) |
| `0x0148` | ROM size |
| `0x0149` | RAM size |
| `0x014D` | Header checksum (validated by boot ROM) |

### MBC Types (`0x0147`)

| Value | Type |
|---|---|
| `0x00` | ROM only |
| `0x01` | MBC1 |
| `0x02` | MBC1 + RAM |
| `0x03` | MBC1 + RAM + Battery |
| `0x0F` | MBC3 + Timer + Battery |
| `0x10` | MBC3 + Timer + RAM + Battery |
| `0x11` | MBC3 |
| `0x13` | MBC3 + RAM + Battery |
| `0x19` | MBC5 |
| `0x1B` | MBC5 + RAM + Battery |

---

## APU Frame Sequencer

Clocked at 512 Hz (falling edge of DIV counter bit 4):

| Step | Length (256 Hz) | Envelope (64 Hz) | Sweep (128 Hz) |
|---|---|---|---|
| 0 | ✓ | | |
| 1 | | | |
| 2 | ✓ | | ✓ |
| 3 | | | |
| 4 | ✓ | | |
| 5 | | | |
| 6 | ✓ | | ✓ |
| 7 | | ✓ | |
