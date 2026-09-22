# Passcode Theme Creator & Universal Flasher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement universal parsing for all `.passthm` archives (fixing missing digits like in MinePass) and create a built-in Passcode Theme Creator with Poster Slicing and Per-Key custom icons in AirCard.

**Architecture:** Python backend (`aircard_backend.py`) parses theme archives using a universal digit/subtext extraction matrix ensuring all iOS 18/17/16 locales find matching cache files. In `AirCardApp.swift`, a native macOS SwiftUI Theme Creator provides interactive 3x4 grid slicing and individual key icon assignment with direct flashing and `.passthm` export capabilities.

**Tech Stack:** Python 3, Swift 5.9, SwiftUI, AppKit / CoreGraphics, ZIP packaging, macOS Sequoia / Darwin.

---

### Task 1: Fix Universal Passcode Flasher in `aircard_backend.py`

**Files:**
- Modify: `aircard_backend.py`
- Test: `tests/test_backend_passthm.py`

- [ ] **Step 1: Write the failing unit test**

Create `tests/test_backend_passthm.py` to verify that `extract_passthm_items` produces all required `en-` and `other-` files for digits 0-9 from both `MinePass_Nightly.passthm` and `тцк.passthm`.

```python
import sys
from pathlib import Path

# Add project root to sys.path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aircard_backend import parse_passthm_archive, KEYPAD_SUBTEXTS

def test_minepass_nightly_extraction():
    minepass_path = "/Users/mak5er/Downloads/MinePass_Nightly.passthm"
    items = parse_passthm_archive(minepass_path, "TelephonyUI-10")
    
    # Verify all digits 0 through 9 are present
    digits_found = set()
    leaves = [item[1] for item in items]
    
    for d in range(10):
        digit_str = str(d)
        has_en = any(f"en-{digit_str}-" in l for l in leaves)
        has_other = any(f"other-{digit_str}-" in l for l in leaves)
        assert has_en, f"Missing en- variant for digit {digit_str}"
        assert has_other, f"Missing other- variant for digit {digit_str}"
        digits_found.add(digit_str)
        
    assert len(digits_found) == 10
    print("✓ MinePass_Nightly parsed all 10 digits successfully")

if __name__ == "__main__":
    test_minepass_nightly_extraction()
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
python3 tests/test_backend_passthm.py
```
Expected: FAIL (`ImportError` or `AssertionError`).

- [ ] **Step 3: Implement universal theme extraction in `aircard_backend.py`**

Define `KEYPAD_SUBTEXTS` and `parse_passthm_archive(passthm_path, telephony_ver)`:
- Extract digit using regex: `r'(?:^[a-zA-Z]+-)?([0-9*#])(?:-([^-\n]+))?'`
- Map to standard subtext if missing.
- Generate complete matrix:
  - `en-{digit}-{subtext}--white.png`
  - `en-{digit}---white.png`
  - `other-{digit}-{subtext}--white.png`
  - `other-{digit}---white.png`
- Use `parse_passthm_archive` in `cmd_flash_passthm` and `cmd_inspect_passthm`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
python3 tests/test_backend_passthm.py
```
Expected: PASS (`✓ MinePass_Nightly parsed all 10 digits successfully`).

- [ ] **Step 5: Commit**

```bash
git add aircard_backend.py tests/test_backend_passthm.py
git commit -m "fix(backend): universal passcode theme parsing for all locales and archives"
```

---

### Task 2: Implement Keypad Slicing & Theme Exporter Engine in Swift

**Files:**
- Modify: `AirCardApp.swift`

- [ ] **Step 1: Implement `KeypadSlicer` and `PasscodeThemeExporter` in `AirCardApp.swift`**

Add utility classes/structs:
- `KeypadSlicer.slicePoster(image: NSImage, zoom: Double, offset: CGPoint) -> [String: NSImage]`:
  - Renders 10 circular crops (digits "0"..."9") at 300x300 pixels with transparent alpha outside circle.
  - Matches 3x4 iOS dialer spacing ratio.
- `PasscodeThemeExporter.exportTheme(keys: [String: NSImage], targetURL: URL) throws`:
  - Generates `TelephonyUI-10/` folder with complete `en-` and `other-` filename matrix.
  - Adds `TelephonyUI-10/_big` marker.
  - Compresses into a `.passthm` zip file.
- `PasscodeThemeExporter.stageTemporaryTheme(keys: [String: NSImage]) -> URL?`:
  - Saves temporary `.passthm` bundle for direct flashing.

- [ ] **Step 2: Verify slicing and export compilation**

Run:
```bash
swiftc -parse AirCardApp.swift
```
Expected: PASS (no syntax or type errors).

- [ ] **Step 3: Commit**

```bash
git add AirCardApp.swift
git commit -m "feat(keypad): add KeypadSlicer and PasscodeThemeExporter engine"
```

---

### Task 3: Build Passcode Theme Creator UI (100% English) in `AirCardApp.swift`

**Files:**
- Modify: `AirCardApp.swift`

- [ ] **Step 1: Add State Variables and Models to `AppViewModel`**

- Add `passcodeTabMode: PasscodeTabMode = .applyTheme`
- Add `creatorSubMode: CreatorSubMode = .posterSlice`
- Add `creatorPosterImage: NSImage?`
- Add `creatorPosterZoom: Double = 1.0`
- Add `creatorPosterOffset: CGPoint = .zero`
- Add `creatorCustomKeys: [String: NSImage] = [:]`
- Add methods:
  - `sliceCurrentPoster()`
  - `setIndividualKeyImage(digit: String, image: NSImage)`
  - `clearCreator()`
  - `flashCreatedTheme()`
  - `exportCreatedTheme(to: URL)`

- [ ] **Step 2: Implement Theme Creator View components**

In `AirCardApp.swift`:
- Top segment: `Picker("", selection: $vm.passcodeTabMode) { ... }` with `[Apply .passthm]` and `[Theme Creator]`.
- In `themeCreatorView`:
  - Sub-mode picker: `[Poster Slice] | [Individual Keys]`.
  - In `Poster Slice`:
    - Image drop area / "Select Poster Image..." button.
    - Zoom slider (`0.5x` to `3.0x`), "Reset Position" button.
    - Interactive 3x4 Keypad preview displaying sliced cutouts.
  - In `Individual Keys`:
    - Interactive 3x4 Keypad grid. Each circular button is an independent drop target and clickable to pick an image for that digit.
  - Bottom action bar:
    - Button "Clear All" (reset creator).
    - Button "Export .passthm..." (opens NSSavePanel in English).
    - Button "Flash to iPhone" (prominent, starts flash).

- [ ] **Step 3: Compile and verify UI structure**

Run:
```bash
./build.sh
```
Expected: Successful compilation into `build/AirCard.app` and `build/AirCard.dmg`.

- [ ] **Step 4: Commit**

```bash
git add AirCardApp.swift
git commit -m "feat(ui): implement interactive Passcode Theme Creator in English"
```

---

### Task 4: End-to-End Testing and Verification

**Files:**
- Test with real device and files:
  - `/Users/mak5er/Downloads/MinePass_Nightly.passthm`
  - `/Users/mak5er/Downloads/AyuGram Desktop/тцк.passthm`
  - Custom generated theme from AirCard Creator

- [ ] **Step 1: Test flashing `MinePass_Nightly.passthm` via backend**

Run:
```bash
python3 aircard_backend.py flash-passthm 00008120-001A1D0A1EE9A01E "/Users/mak5er/Downloads/MinePass_Nightly.passthm" TelephonyUI-10
```
Expected: All 10 digits flash successfully (no missing 2-9 keys).

- [ ] **Step 2: Deploy updated app to `/Applications/AirCard.app`**

Run:
```bash
rm -rf /Applications/AirCard.app && cp -R build/AirCard.app /Applications/AirCard.app && xattr -cr /Applications/AirCard.app
```
Expected: App runs smoothly, loads both tabs, switches between Apply .passthm and Theme Creator.

- [ ] **Step 3: Final Commit & Clean up**

```bash
git add .
git commit -m "chore: finalize Passcode Theme Creator and verified build v1.2"
```
