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
| Select | S | Click to pick, Shift-click to add, drag to move, drag on empty board to box (left→right takes what's inside, right→left anything touched). Grab a track near its end to move just that end. Double-click a track to pick up everything joined to it. Moving keeps 45° work at 45° (see below). |
| Route | W | Click a pad, via or track to start. Routing works like Altium's: see below. |
| Via | V | Click to drop a via; it takes the net of what it lands on. |
| Outline | O | Click the board's corners; click the first corner (or Enter) to close. |
| Pour | P | Click a copper pour's corners on the layer in use; click the first corner (or Enter) to close. It takes the lit-up net, or the net under its first corner, or GND. See below. |
| Keepout | K | Click an area's corners on the layer in use; click the first corner (or Enter) to close. Pours keep out of it. See below. |
| Place | (click a footprint in the library) | Click to place, R turns, L puts it on the other side, Esc stops. |

### Names on the silkscreen

Click **Top silkscreen** or **Bottom silkscreen** in the layer list (or step to
them with `+` and `−`, which cycle through the copper layers and then the two
silkscreens) to work on that layer: it gets the *working* tag, and a part's name
(its designator, such as R25) can be picked and dragged. Drag one and only it
moves; its card on the right sets the name, its height and puts it back in the
usual place above the part. A name follows its part when the part is moved, turned
or flipped to the other side.

Names are all the board's *Name height* (in the rules) unless one was given a
height of its own, which a board from Altium has where a name wasn't the usual
size; *Make the odd-sized names this tall* on the right gives them all the
board's.

### Copper pours and planes

A pour fills its outline with copper on one layer, keeping its clearance from
everything of other nets, from holes and from the board's edge. Pads of its
own net join it solid or by thermal relief (four spokes across a gap round
the pad). Pieces that touch nothing of its net, pieces smaller than a set
area, and necks narrower than a set width are taken out. Pours go down in
their pour order, each later one keeping clear of those before it.

Pours pour again by themselves whenever a change is finished; while a part
is dragged or a route is laid they keep their old shape, and the router and
pushing take no notice of them, as they'll make way afterwards. Pick a pour
by its dashed outline, or click inside it; its card on the right sets the
net, layer, clearances, how pads join, and the rest. Dragging a pour moves it.

A **power plane** is a pour that ticks *Power plane* (and usually *Covers
the whole board*) on an inner layer: it keeps clear of other nets' holes by
its clearance measured from the hole, as Altium's planes do, and keeps every
piece. Planes are written as ordinary positive copper layers.

A **fill** is copper of a fixed shape (an Altium fill or solid region).
Routing treats it like any other copper.

Gerbers write each poured piece as one region, its holes joined to the
outline by cut-ins.

### Keepouts

A **keepout** is an area kept free of copper on one layer, or on every layer.
Drawn with the Keepout tool it keeps **pours** out and nothing else: pours
stop short of it by their clearance, and are poured again when it changes.
Its card on the right says what else it keeps out (tracks, vias, surface pads,
through-hole pads) and on which layer; the router goes round a keepout that
keeps tracks out, and the rule check flags tracks, vias and pads in one that
keeps them out ("In a keepout"; touching its edge is allowed). A part can carry
keepouts of its own, which move, turn and flip to the other side with it; a part's
own pads aren't held to its own keepouts. Keepouts aren't written to the Gerbers:
they only shape what is.

### Moving keeps the corners

On a board laid at 45°, moving things doesn't leave tracks at odd angles:

- Drag one straight track and it slides sideways, keeping its direction.
  Its neighbours keep theirs too, growing or shrinking to meet it; where the
  track ends on a pad or via, a short 45° leg joins it back. Other nets'
  tracks it runs into are pushed aside (and their vias, with Push vias too
  on), cascading and springing back as in routing. It won't come closer than
  the clearance to pads, holes or the edge: it stops as close as it can get,
  and the status bar says why. When you let go, the rule check runs, and if
  it finds anything new too close the slide is taken back.
- Move a part, a via, or a group, and each track joined to it keeps its
  direction while the next corner slides along the following track; a track
  with nowhere to slide is re-laid as a straight leg and a 45° leg.
- Drag the corner where tracks meet and they're re-laid from their far ends
  in 45° legs.
- Pushed tracks keep 45° too.

### Moving pushes

Anything you drag pushes other nets' tracks out of its way, and their vias
with Push vias too on: a part's pads, a via, a slid track, tracks moved
together, a dragged corner or track end. Turning (R) and flipping (L) a part
push too. Pushes cascade, keep 45°, and spring back as you drag back.

Nothing that moves comes within the clearance of what can't be pushed (other
parts' pads, holes, the board edge): the drag stops as close as it can get
and the status bar says why. When you let go, the rule check runs, and if it
finds anything new too close to something you didn't move, the move is taken
back. A part's own tracks tangling with its pads after a turn is left for you
to tidy, and the checks show where. Hold **Ctrl** while
dragging to move freely without pushing; the rule check then just shows
anything that's too close.

Tracks that weren't at a multiple of 45° to begin with just stretch, so
any-angle work stays as it was drawn.

Arc corners stay arcs. Slide a straight track that runs into a rounded
corner and the arc keeps its radius and stays tangent, while the straight
track beyond it grows or shrinks along its own line. Drag an arc itself and
its radius changes, staying tangent to the tracks either side: pull it out
for a bigger radius, in for a smaller one, never tighter than the Shortest
45° rule. Moving a part or via does the same for the tracks joined to it: a
straight track into a rounded corner keeps its direction while the corner
moves along the track beyond, and an arc starting at a pad moves with the
pad while what's beyond it follows, still tangent.

### No square corners

The board rule **Shortest 45°** (0.5 mm unless you change it, in the Rules
card) keeps 45° work from closing up into right angles:

- Routing at 45°, a turn of 90° or more is cut with 45° steps at least that
  long (one for a right angle, two for a sharper turn), and a 45° step that's
  too short is lengthened, wherever the clearance allows. That includes the
  corner at a click point, which may shorten the piece placed before it.
  Where there's no room to cut, the short stretch is re-laid as a straight
  leg and a 45° leg.
- Moving, a square corner a move makes is cut the same way, and a move that
  would squeeze a 45° step below the rule stops there instead.
- Pushed tracks get the same treatment at the corners a push makes.

The 90° and arc corner styles keep their corners: that's what they're for.

### Routing

While routing, the router shows what a click would lay from the route's end
to the pointer. A click places everything except the last, hollow segment
(the look-ahead), so you set where a corner goes without committing to what
comes after it. Landing on a pad, via or track of the same net places the lot
and ends the route.

| Key | While routing |
|---|---|
| Shift+Space | Corners: 45°, 45° with arc, 90°, 90° with arc, any angle |
| , and . | Arc radius down and up a mil (0.0254 mm); with Shift, ten mils |
| Space (or /) | Flip which way the corner goes |
| Shift+R | When something's in the way: walk around it, push it aside, hug and push, stop at it, or ignore it (shows what it would run over, outlined red, but still only places what's clear) |
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

**Push** goes round what can't move (pads, vias, holes, the board edge) and
through other nets' tracks, which bend out of its way. Each track is bent as
little as it can be, its ends stay where they are, and a pushed track pushes
whatever it then runs into. Move the pointer away and they spring back; a
click keeps them. If a track can't give way (pinned between pads, or it
would have to go the long way round) the route walks round instead and the
routing chip says so. Backspace and Ctrl+Z put pushed tracks back too.

With **Push vias too** on (it is by default), a via in the way moves the
least it can to a spot that's clear of what can't move on every layer, and
its tracks on every layer follow it with a short jog. On the other layers the
moved via and the tracks that followed it push those layers' tracks aside in
turn, layer after layer until everything settles; the route being laid shows
the board as it would be left, every layer included. If another layer can't
make room, the via stays put and the route goes round or pushes otherwise. Only the
stretch of a pushed track near the trouble changes; the rest stays exactly
where it was.

**Hug and push** keeps close round what's in the way while that's a fair
path, and pushes only when going round would be the long way or no way.

**Arc corners** plan the route with sharp corners, then round each into a
tangent arc of the arc radius (shown in the routing chip). A corner's arc can
use up to half of each segment it shares with another corner. An arc that
would come too close to something tries half the radius, and so on, or the
corner stays sharp. Arcs are real arcs on the board and in the Gerbers
(G02/G03); moving their ends keeps their shape. Arcs are pushed like any
other track: a run with arc corners is bent as if each arc were the corner it
rounds off, then its corners are rounded again — the old ones at their old
radii, new ones at the run's, each shrinking if it would come too close.

**The clearance always holds.** Every mode plans clear paths, and before
anything is placed (a route, a pushed track, a moved via, a dropped via)
the rule check is run on the result: if it would call anything too close,
shorted or too near the edge, nothing is placed and the status bar says why.

While dragging a part, **Space** turns it 90° (Shift+Space the other way),
as do R and Shift+R, and L puts it on the other side; the drag carries on
with it turned. Space and R turn a part you're placing from the library too.

Anywhere: R turn 90° (Shift+R the other way), L other side, Delete remove,
Ctrl+D copy, Ctrl+A everything, N light up the net under the pointer, 1–6 or
+/− or PgUp/PgDn change copper layer, F fit, X see it from underneath,
G grid, U units, Ctrl+Z / Ctrl+Shift+Z undo and redo, Ctrl+S save (⌘ on a Mac for all the Ctrl keys),
Ctrl+O open, Ctrl+N new, Ctrl+E write Gerbers. Scroll or pinch to zoom,
right- or middle-drag to pan.

The rule check runs all the time: problems are marked on the board and listed
on the right; click one to go to it.

## Files

Boards are saved as `.cspcb`, which is plain JSON (millimetres, Y up). An
arc track has a `mid` point on the way from `a` to `b`, as KiCad does. Parts on
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

### Altium and Circuit Studio libraries

Coppersmith reads Altium Designer and Circuit Studio libraries: `.PcbLib`
footprints, `.SchLib` symbols, and `.IntLib` integrated libraries (which hold
both). Add one with the **+** button above the footprint list: it's copied
into your footprint folder and its footprints appear under the library's
name, ready to place. A footprint takes its name letter (R, C, U…) from the
symbol that uses it, when the library has symbols.

Pads (round, oval, rectangular and rounded; through-hole, plated or not),
silkscreen, and courtyard and assembly drawings on mechanical layers come
across. Octagonal pads become rounded rectangles and slotted holes round
ones; copper drawn outside pads (copper tracks, fills, custom pad shapes) is
left out — the library list says how many footprints were approximated,
with the details on hover.

    coppersmith --read-altium LIBRARY.IntLib

lists what's in a library, symbols included (pins, prefix, linked footprint),
and what was left out. Symbols are read now and kept for the schematic
editor still to come. Writing Altium libraries is next.

### Altium and Circuit Studio boards and schematics

**Open** also takes `.PcbDoc` and `.CSPcbDoc` boards. The board comes in
unsaved, as a new Coppersmith board: outline, copper layers, parts (with
their footprints as placed), nets, tracks, arcs and vias, fills and solid copper regions, and the design rules (clearance from the
clearance table, track widths, via sizes, mask and edge clearance).

Keepouts come in as keepouts: fills, regions and tracks flagged as keepouts,
on the board and on parts, and shapes on the Keep-Out layer. They keep pours
out and nothing else, since tracks, vias and pads are wanted there; one whose
restrictions don't include copper pours is left out. A shape on the Keep-Out
layer that covers most of the board is the board's boundary, not a keepout.

Pours come in as pours, from their outlines and settings (pour order,
dead copper and small-island removal, neck width), with the clearance the
rules give polygons and the polygon-connect rule's direct or relief joins,
and are poured again here rather than copied, so they stay live. Plane
layers become plane pours: one per split plane, or the whole board for a
plane layer with a net and no splits, with the plane clearance and
plane-connect rules and the layer's pullback from the edge. Hatched pours
are poured solid; pour cutouts, shelved pours and free pads are left out.
The status bar says what was left out. Part names come in where Altium had
them and as tall as it drew them: the usual height becomes the board's name
height, and a name of any other height keeps its own.

    coppersmith --read-altium BOARD.CSPcbDoc
    coppersmith --read-altium SHEET.SchDoc
    coppersmith --read-altium PROJECT.PrjPcb

summarises a board (with its rules), a schematic sheet (parts, wires,
labels, ports), or a whole project: every sheet's netlist joined up and
checked net by net against the board, listing any pin the two disagree on.

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
        pour.rs        pouring copper pours and planes
        drc.rs         the rule check
        router.rs      the interactive router: corner styles, walk round, push, arcs, loops
        slide.rs       moving tracks without losing their 45° corners
        gerber.rs      Gerber and Excellon output
        altium/        reading Altium / Circuit Studio libraries, boards, sheets, projects
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
