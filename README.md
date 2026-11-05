# Coppersmith

Lay out printed circuit boards: place parts, route copper, check the rules,
and write Gerbers. It wears Copperview's clothes and hands its output straight
to it.

    coppersmith                     open the window with the sample board
    coppersmith BOARD.cspcb         open a board
    coppersmith BOARD.cspcb --gerbers DIR   write Gerbers and drill files, no window
    coppersmith BOARD.cspcb --check         print the rule check, no window

`coppersmith --help` lists every option.

## Working on a board

| Tool | Key | What it does |
|---|---|---|
| Select | S | Click to pick, Shift-click to add, drag to move, drag on empty board to box (left→right takes what's inside, right→left anything touched). Grab a track near its end to move just that end. Double-click a track to pick up everything joined to it. |
| Route | W | Click a pad, via or track to start, click to turn corners (45°), V drops a via and changes side, `/` swaps which leg is diagonal, Backspace steps back, Esc or right-click finishes. Arriving on a pad of the same net finishes by itself. |
| Via | V | Click to drop a via; it takes the net of what it lands on. |
| Outline | O | Click the board's corners; click the first corner (or Enter) to close. |
| Place | (click a footprint in the library) | Click to place, R turns, L puts it on the other side, Esc stops. |

Anywhere: R turn 90° (Shift+R the other way), L other side, Delete remove,
Ctrl+D copy, Ctrl+A everything, N light up the net under the pointer, 1–6 or
+/− or PgUp/PgDn change copper layer, F fit, X see it from underneath,
G grid, U units, Ctrl+Z / Ctrl+Shift+Z undo and redo, Ctrl+S save,
Ctrl+O open, Ctrl+N new, Ctrl+E write Gerbers. Scroll or pinch to zoom,
right- or middle-drag to pan.

The rule check runs all the time: problems are marked on the board and listed
on the right; click one to go to it.

## Files

Boards are saved as `.cspcb`, which is plain JSON (millimetres, Y up). Parts on
a board carry a copy of their footprint, so a board file stands on its own.

Gerbers are RS-274X with X2 attributes and Altium's file endings (GTL, GBL,
G1…, GTS, GBS, GTP, GBP, GTO, GBO, GKO) plus `-PTH.drl` and `-NPTH.drl`
Excellon files, which every board maker and Copperview read. After writing
them, **View in Copperview** opens them there.

## Footprints

The common packages are built in, made from their datasheet sizes: chip
resistors, capacitors and LEDs (0402–1206), SOIC, TSSOP, QFN, DIP, SOT-23,
SOD-123, TO-220, pin headers, screw terminals, a tactile switch, a 3225
crystal, mounting holes and a test pad.

Your own go in `~/.local/share/coppersmith/footprints/*.json` (the folder
button above the library opens it), one footprint or a list per file, in the
same shape as a footprint in a board file. A file can replace a built-in
footprint by using its name.

## Building and checking

    cargo build --release
    cargo test
    coppersmith --screenshot out.png --zoom 25,18,30 --play 'key w; bclick 27.475 16.095; bclick 29 9.0875'

`--play` acts out mouse and key steps; `bclick X Y` and friends take board
millimetres, so scripts keep working whatever size the window is.

Figtree is included under the SIL Open Font License (see assets/fonts/OFL.txt).
