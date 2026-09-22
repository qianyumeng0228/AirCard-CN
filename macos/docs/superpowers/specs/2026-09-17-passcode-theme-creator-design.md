# AirCard Passcode Theme Creator & Universal Flasher Design Spec

## 1. Overview
This specification details the architecture, data models, UI components, and implementation logic for:
1. **Universal Passcode Flasher (`aircard_backend.py`)**: Fixing compatibility with legacy/multi-lingual themes (such as `MinePass_Nightly.passthm` with `ru-` prefixes and varying subtext configurations) to ensure 100% reliable flashing regardless of device language or source archive structure.
2. **Built-in Theme Creator (`AirCardApp.swift`)**: A visual editor inside the "Passcode" tab with two distinct creation modes:
   - **Poster Slice (Puzzle)**: Slicing a single wallpaper/image across the authentic 3x4 iOS lockscreen passcode keypad geometry.
   - **Individual Keys (Per-Key)**: Customizing each digit (0–9) individually with drag-and-drop or file pickers.
3. **Actions**: Instant flashing to connected iOS devices via `airlift` and exporting to standard `.passthm` zip archives.
4. **Localization Rule**: The macOS app UI text and labels are 100% in English.

---

## 2. Universal Flasher Fix (`aircard_backend.py`)

### 2.1 Root Cause of Flashing Failures
- Archives like `MinePass_Nightly.passthm` contain files named `ru-2-A B C--white.png` instead of `en-2-A B C--white.png`.
- Key digits 0 and 1 have no subtext letters (`ru-0---white.png`, `ru-1---white.png`), so generic replacements worked for 0 and 1 only.
- Digits 2–9 failed because iOS looks for `en-{digit}-{letters}--white.png` or `other-{digit}-{letters}--white.png` depending on system locale.

### 2.2 Standard Subtext Table
```python
KEYPAD_SUBTEXTS = {
    "0": "+",
    "1": "",
    "2": "A B C",
    "3": "D E F",
    "4": "G H I",
    "5": "J K L",
    "6": "M N O",
    "7": "P Q R S",
    "8": "T U V",
    "9": "W X Y Z",
}
```

### 2.3 Extraction & File Generation Matrix
For every valid button image in the archive targeting key digit `D` (0–9) and subtext letters `LETTERS`:
1. Keep the original filename from the archive.
2. Standard English with letters: `en-{D}-{LETTERS}--white.png` (if `LETTERS` is non-empty).
3. Standard English without letters: `en-{D}---white.png`.
4. Other locale with letters: `other-{D}-{LETTERS}--white.png` (if `LETTERS` is non-empty).
5. Other locale without letters: `other-{D}---white.png`.
6. Standard Keypad Subtext fallback: If the archive had no letters or non-standard letters, also generate `en-{D}-{KEYPAD_SUBTEXTS[D]}--white.png` and `other-{D}-{KEYPAD_SUBTEXTS[D]}--white.png`.

Destination directory: `/var/mobile/Library/Caches/{telephony_ver}` (e.g. `TelephonyUI-10` for iOS 18+, `TelephonyUI-9` for iOS 15–17).

---

## 3. Passcode Theme Creator Architecture

### 3.1 Data Model
In `AirCardApp.swift`:
```swift
enum PasscodeTabMode: String, CaseIterable, Identifiable {
    case applyTheme = "Apply .passthm"
    case themeCreator = "Theme Creator"
    var id: String { rawValue }
}

enum CreatorSubMode: String, CaseIterable, Identifiable {
    case posterSlice = "Poster Slice"
    case individualKeys = "Individual Keys"
    var id: String { rawValue }
}

struct KeypadButtonGeometry {
    let digit: String
    let letters: String
    let row: Int
    let col: Int
}
```

### 3.2 Keypad Layout Constants
- Grid: 3 columns, 4 rows.
- Standard key placement:
  - Row 0: `1` (col 0), `2` (col 1), `3` (col 2)
  - Row 1: `4` (col 0), `5` (col 1), `6` (col 2)
  - Row 2: `7` (col 0), `8` (col 1), `9` (col 2)
  - Row 3: `0` (col 1)
- Aspect ratios and spacing match authentic iOS Lock Screen dialer:
  - Key diameter: 75 pt
  - Horizontal spacing: 24 pt
  - Vertical spacing: 18 pt
  - Total grid width: (3 * 75) + (2 * 24) = 273 pt
  - Total grid height: (4 * 75) + (3 * 18) = 354 pt

### 3.3 Slicing Engine (`Poster Slice Mode`)
- The user provides an image (`NSImage`).
- Slicing Math:
  - Scale image to fit or fill the keypad bounding box.
  - Calculate normalized center `(cx, cy)` and radius `r` for each of the 10 buttons.
  - Render a circular mask at high resolution (300x300 pixels for `@3x` Super Retina display).
  - Produce 10 separate circular `NSImage` instances for digits `0` through `9`.
- User controls:
  - Drag & drop image target.
  - Zoom slider (0.5x to 2.5x) and Offset X/Y adjustment or drag to re-frame.
  - Real-time interactive preview showing circular cutouts over the image.

### 3.4 Individual Keys Mode
- 3x4 grid representation.
- Each button has an independent image drop target / click-to-browse button.
- User can set custom images for individual digits, or clear any individual digit.
- Missing digits fall back to transparent or standard numeric glyphs.

### 3.5 Direct Flash & Export Actions
1. **Flash to iPhone**:
   - Compiles the current 10 images into temporary PNG files in an in-memory or temporary `.passthm` directory.
   - Triggers `cmd_flash_passthm` in `aircard_backend.py`.
   - Uses device connection detection and updates progress bar step-by-step.
2. **Export .passthm**:
   - Opens an `NSSavePanel` in English ("Save Passcode Theme").
   - Creates a standard zip package containing:
     - `TelephonyUI-10/` with all mapped `en-` and `other-` files.
     - `_big` or `_small` marker file.
   - Saves file with `.passthm` extension.

---

## 4. UI Design & Layout (English)

### 4.1 Passcode Tab Header
- Segmented picker at the top: `[Apply .passthm] | [Theme Creator]`.

### 4.2 Theme Creator Screen
- Top Control Bar:
  - Sub-mode picker: `[Poster Slice] | [Individual Keys]`.
  - Actions: `Reset / Clear All`, `Export .passthm...`, `Flash to iPhone`.
- Content Area:
  - In **Poster Slice** mode:
    - Left/Top: Image import drop zone with controls (`Select Image...`, `Zoom`, `Fit / Fill`).
    - Right/Center: Interactive iOS Lockscreen Keypad preview displaying the framed poster seamlessly through the 10 circular cutouts.
  - In **Individual Keys** mode:
    - Full 3x4 grid with circular buttons. Clicking any circle opens an image picker; dragging an image over any circle immediately applies it to that key.
- Bottom Status Bar:
  - Integrated with the existing progress bar and activity log for real-time flash status.

---

## 5. Testing & Verification Plan
1. **Theme Compatibility Test**:
   - Test flash on `MinePass_Nightly.passthm` -> verify all 10 digits (0–9) generate `en-` and `other-` variants and flash successfully to `TelephonyUI-10`.
   - Test flash on `тцк.passthm` -> verify it continues to work 100%.
2. **Poster Slicing Test**:
   - Load arbitrary 16:9 and 19.5:9 wallpapers.
   - Verify all 10 cropped PNGs are generated at 300x300, circular, and transparent outside the mask.
3. **Individual Keys Test**:
   - Assign different images to key 1, 2, and 0.
   - Flash to device and export to `.passthm`.
   - Inspect the exported zip archive structure to ensure compliance with iOS cache standards.
