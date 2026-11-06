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
| Route | W | Click a pad, via or track to start. Routing works like Altium's: see below. |
| Via | V | Click to drop a via; it takes the net of what it lands on. |
| Outline | O | Click the board's corners; click the first corner (or Enter) to close. |
| Place | (click a footprint in the library) | Click to place, R turns, L puts it on the other side, Esc stops. |

### Routing

While routing, the router shows what a click would lay from the route's end
to the pointer. A click places everything except the last, hollow segment
(the look-ahead), so you set where a corner goes without committing to what
comes after it. Landing on a pad, via or track of the same net places the lot
and ends the route.

| Key | While routing |
|---|---|
| Shift+Space | Corners: 45°, 90°, any angle |
| Space (or /) | Flip which way the corner goes |
| Shift+R | When something's in the way: walk around it, stop at it, or ignore it (the rule check then shows the clash; what it runs over is outlined red) |
| Ctrl+click | Finish the connection by itself, round whatever is in the way |
| Ctrl | Hold to stop the pointer snapping to pad centres |
| V | Via, and carry on on the other side |
| 2 | Via here, same layer |
| + − PgUp PgDn | Change layer, dropping a via |
| 1 | Look-ahead segment on or off |
| 3 | Width from: your choice → rule minimum → rule preferred |
| Shift+D | Take out old paths a new route replaces (on by default) |
| Ctrl+W | Shade the clearance round other nets |
| Backspace | Take back the last click |
| Enter | Same as a click |
| Esc, right-click, double-click | End the route, keeping what's placed |

The same settings are in the routing pill on the toolbar. Walking round is
an A* search on a fine grid kept clear of other nets and the board edge,
then pulled straight wherever an exact clearance check allows.

Anywhere: R turn 90° (Shift+R the other way), L other side, Delete remove,
Ctrl+D copy, Ctrl+A everything, N light up the net under the pointer, 1–6 or
+/− or PgUp/PgDn change copper layer, F fit, X see it from underneath,
G grid, U units, Ctrl+Z / Ctrl+Shift+Z undo and redo, Ctrl+S save (⌘ on a Mac for all the Ctrl keys),
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

Your own go in a `footprints` folder in Coppersmith's data folder (the folder
button above the library opens it):

| | |
|---|---|
| Linux | `~/.local/share/coppersmith/footprints/` |
| macOS | `~/Library/Application Support/coppersmith/footprints/` |
| Windows | `%APPDATA%\coppersmith\footprints\` |

Put one footprint or a list per file, in the
same shape as a footprint in a board file. A file can replace a built-in
footprint by using its name.

## Building

Coppersmith is plain Rust (1.88 or newer) and builds the same way on Linux,
macOS and Windows. Get Rust from https://rustup.rs, then:

    cargo build --release        # the program is target/release/coppersmith(.exe)
    cargo test

What each computer needs besides Rust:

| | Needs | Package it |
|---|---|---|
| Linux | a C linker; on Debian/Ubuntu also `libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev` | `sh packaging/linux/install.sh` adds it to ~/Applications and the launcher |
| macOS | Xcode command line tools (`xcode-select --install`) | `sh packaging/macos/make-app.sh` makes `target/Coppersmith.app` (`--universal` for Apple Silicon + Intel) |
| Windows | Visual Studio Build Tools, "Desktop development with C++" | `packaging\windows\make-zip.ps1` makes a zip with `coppersmith.exe` |

`.github/workflows/build.yml` builds, tests and packages all three on GitHub.

### Where things are

    src/
      main.rs        command line, opening the window
      pcb/           the board and everything worked out from it — no windows, no OS
        geom.rs        points and the grown-shape sums behind clearance and picking
        model.rs       the board, parts, pads, tracks, vias, nets, rules; the .cspcb file
        library.rs     built-in footprints and your footprint files
        copper.rs      what touches what, and the still-to-connect lines
        drc.rs         the rule check
        router.rs      the interactive router: corner styles, walk round, stop, loops
        gerber.rs      Gerber and Excellon output
        font.rs        the silkscreen stroke font
        sample.rs      the 555 sample board
      ui/            the window
        app.rs         editor state, undo, edits, files, keys
        routing.rs     laying a route: clicks, look-ahead, vias, auto-complete
        view.rs        drawing the board and what the mouse does
        panels.rs      top bar, side panels, status bar
        theme.rs       Copperview's colours and fonts
        icons.rs       line icons, the app picture
        play.rs        scripted input for picture checks
      platform/      what differs between Linux, macOS and Windows
        mod.rs         data folder, opening folders, finding programs, install, console
        pinch_wayland.rs  trackpad pinches on Linux (macOS and Windows get them free)
        install_linux.rs  ~/Applications and the launcher entry
    assets/          fonts (built in, so it looks the same everywhere) and icons
    packaging/       per-platform install and bundle scripts

Anything that touches the operating system goes in `platform/`, so `pcb/`
and `ui/` stay the same on every computer.

### Checking by picture

    coppersmith --screenshot out.png --zoom 25,18,30 --play 'key w; bclick 27.475 16.095; bclick 29 9.0875'

`--play` acts out mouse and key steps; `bclick X Y` and friends take board
millimetres, so scripts keep working whatever size the window is. Plain
`click X Y` takes window points (screenshot pixels divided by the display
scale). `hold ctrl` … `release` holds a key across clicks.

Figtree and JetBrains Mono are included under the SIL Open Font License
(see assets/fonts/OFL.txt and assets/fonts/OFL-JetBrainsMono.txt).
