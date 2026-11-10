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

### The window

Everything is in the menus along the top: **File**, **Edit**, **View**,
**Place**, **Route**, **Tools** and **Help**, each entry with its shortcut
beside it (**Help ▸ Keyboard shortcuts**, F1, lists them all).

The panels either side are chosen from **tabs along their bottoms**. On the left,
*Layers* (what to show, which layer to work on), *Parts* (the footprint library)
and *Select* (what can be picked); on the right, *Properties* (the details of what's
picked, or the board's and its rules), *Checks* and *Nets*. A badge on a tab
says something's there: the number of errors on Checks, and a yellow one on
Select when something is held back. Picking something on the board brings
Properties forward. Drag a panel's edge to size it; hide the left or right panel
with the arrows at the top right, **View ▸ Left panel / Right panel**, or
Ctrl+[ and Ctrl+].

At the top of the *Layers* list are quick choices for the copper: **All**, **Top**,
**Bottom** and **Planes** (every inner plane), and on a second row one chip for
each inner layer that carries signals (**In 2**, **In 5**…), which shows just that
layer and works on it. Which inner layers are planes and which are for signals is
worked out from the board whenever it changes: a layer with a plane pour, or a
pour over most of the board and next to no tracks, is a plane; anything else
inside is a signal layer. Silkscreen and the other layers keep as you set them.

The tools, the layer, track width, grid and view buttons float over the board.
**View ▸ Ribbon of controls** (Ctrl+Shift+R) puts them in a strip under the menus
instead, with file and edit buttons, the select filter and the tools in groups;
in a narrow window it scrolls sideways. The layout (panels shown, their tabs, the
ribbon, the select filter) is remembered between runs, in `window.json` in
Coppersmith's data folder.

**Selecting what you mean.** The select filter says which kinds of thing can be
picked: parts, names, tracks, vias, pours, fills and keepouts. Click a kind's switch
on the *Select* tab to turn it off or on, *only* to take just that kind, or use the
presets (*All*, *Copper*, *Parts*, *None*). The same controls are in the chip at
the bottom right, which turns yellow when something is held back, and in
**Edit ▸ Selection filter**. A kind that's off is left alone, so a click goes through
it to what's under it, boxes and Select all skip it, and a click that finds only
held-back things says what it found.

    coppersmith BOARD.cspcb --layout 'ribbon, no-left, right:checks, filter:tracks+vias'

opens the window in a given layout, for scripted pictures; such a run neither
reads nor writes the remembered one.

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

Zoomed in, each net's name is written along its tracks on the working layer,
turned to follow them and kept the right way up, on every straight track long
enough to hold it (the part on the screen, for a long one). View ▸ *Net names on
tracks* turns it off.

The rule check runs all the time: problems are marked on the board and listed
on the right; click one to go to it.

**Design rules.** *Rules ▸ Design rules…* opens a window with every rule
Altium's files use (36 kinds, in Altium's groups: electrical, routing, high
speed, manufacturing, placement, planes and pours, mask, surface mount, test
points, supplies), each with what it asks and its controls. A rule is
*Checked* (a switch turns its check on and off, and the number it asks for is
there to change), *Applied* (used when pours and Gerbers are made) or *Shown
only* (read from the file and kept, not checked yet), and what the file says of
each (its scope, its settings) is listed under it. The checks: clearance and
short circuit, un-routed nets, track width (least and most), the width a net
class asks of each layer, hole sizes (least and most), annular ring, hole to
hole, board outline, component clearance (overlap, and a gap), keep-outs,
matched lengths within a pair or across a class, net length, acute angles, and
un-poured polygons. From an Altium file a rule it doesn't have or has switched
off starts off, as in Altium; the via style rule (for through vias only, which a
board here can't tell from blind ones), hole to hole, acute angle and length
start off on a new board.

Click a marker on the board, or a problem in the list: the Checks panel shows
the problem, the **rule it breaks** and that rule's controls (for a class
width or matched-length problem, the rules of that net's class), so it can be
changed on the spot, and one undo takes it back.

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
and what was left out. Symbols are kept for the schematic editor still to come.

### The library editor

A **LIB** tab (**+ ▸ New library**, a library from the Project panel or File ▸
Open…, or `coppersmith --library FILE.PcbLib`) is an editor for Altium
libraries, and reads and writes the same
files: footprints as a `.PcbLib`, symbols as a `.SchLib`. Open an `.IntLib` and
both come in; save writes each kind to its own file.

- **Footprints.** Pads (shape, size, hole, turn, number), and lines and circles
  on the silkscreen, courtyard and assembly layers. Click a tool, then the
  drawing; the selection takes its details in the right panel; drag to move
  on the grid; R turns; Delete removes; Ctrl+Z undoes. *Pad patterns* makes two
  rows numbered as a DIP or SOIC is (with an outline and a pin 1 mark), or a
  row of pads. **Save to my footprints** writes a PcbLib into the folder the
  Parts list reads, so the footprints can be placed straight away.
- **Symbols.** Pins (number, name, electrical type, length, direction, hidden),
  and lines and rectangles for the body, on a 100 mil grid, with many-part
  symbols and the footprints a symbol links to (it says when one isn't in a
  library that's open).

Not yet: editing keepouts, text and arcs (they're read, shown and kept, but not
drawn with a tool), 3D bodies, and picking more than one thing. What's written
is read back by Coppersmith's own reader in tests, for the real test libraries
as well as made-up ones; **it hasn't been opened in Altium Designer or Circuit
Studio**, which fill in the bookkeeping they keep (unique ids, embedded models)
when they next save.

### Tabs, projects and the menus

**Tabs.** Everything that's open has a tab along the top: the board (**PCB**,
always first) and each schematic (**SCH**) and library (**LIB**) you open, as many
as you like. Click a tab to show it (each keeps its place and its undo); the **×**
on a tab, middle-click or Ctrl+W closes it (one with unsaved changes asks to be
closed a second time, so nothing goes by accident; a dot after the name means
unsaved); **+** starts a new schematic or library or opens a file. Opening a file
that's already open just shows its tab. Double-clicking a part in the Parts list
opens its library in a tab.

**Projects.** The **Project** tab, first of the tabs along the bottom of the left
panel in every window (on a schematic or library tab it sits beside that editor's
own list: Symbols, or Footprints), lists the schematics, boards and libraries that
go together;
**double-click** one to open it (a board in the PCB tab, a sheet or a library in a
tab of its own; files already open say so; right-click for *Show in its folder*).
**File ▸ Open project…**, **File ▸ Open…** and `coppersmith PROJECT.PrjPcb` read an
Altium or Circuit Studio project file (`.PrjPcb`, `.CSPrjPcb`) and list the
documents it names (outputs and reports are left out; a missing file is shown as
such). Open any board, sheet or library and the project it belongs to is found the
same way: the project file in its folder that lists it, or else the folder's files.

**Menus follow the tab.** On a schematic or library tab the menus are about what
it edits: File has New, Open, Save, Save as…, Open project… and Close this tab (no
Gerbers; *Save to my footprints* only for footprints); Edit has Undo, Redo, Delete,
Turn and (sheets) Mirror; View has Fit and the left panel; Place lists the
drawing tools. Route and Tools appear only on the PCB tab. On the board,
*Add footprints to the Parts list…* is the old "add a library", and *New library*
and *New schematic* add tabs.

### The schematic editor

**+ ▸ New schematic** (or `coppersmith --schematic FILE`) opens a sheet to
draw on. The left panel (the *Symbols* tab) has two pages, and the tool you pick
brings its page forward:

- **Markers**, for **Net label** and **Power**: the standard things that aren't
  parts, each drawn as it will look on the sheet. *Power and ground:* GND, AGND
  (signal ground), PE (earth), VCC, +3V3, +5V, +12V, −12V, VDD, VBAT, VIN, VAC,
  in Altium's styles (power ground, signal ground, earth, bar, circle, arrow,
  wave); a name typed over takes the look its name usually has. *Nets:* a net
  label, a bus (`NAME[0..7]`), and a **differential pair**, which puts down
  `NAME_P` and `NAME_N` a grid apart in one click (the number counts up for the
  next). *Ports:* input, output, bidirectional, unspecified. A picked supply or
  port can change style or direction in the card on the right.
- **Parts**, for **Part**: the symbols of the **project's libraries**, the
  `.SchLib` and `.IntLib` files its project lists (read once and kept; the ones
  open in a tab stand in with their edits). Choose one library or all, search,
  and see the symbol under the pointer drawn before you place it. **+** opens
  another library for that sheet. Nothing is built in or kept in a data folder:
  with no project, the list is empty until you open one.

- **Part:** pick a symbol, click to put it down, again for more; R turns it, X
  mirrors it, Esc stops. Designators count up (R1, R2…).
- **Wire:** click the corners; it draws in right angles (Space picks which way
  the corner bends) and ends itself when it lands on a pin or another wire; Esc
  ends it anywhere. Junction dots appear where a wire meets another partway.
- **Net label** and **Power:** pick a marker, type a name, click a pin or wire. A
  label with a number at the end counts up (D0, D1…); a power port points away
  from its pin (a ground hangs down when it's on its own) and joins every port
  of the same name.
- **Looks:** everything on the sheet is sized in millimetres (strokes, pin
  numbers and names, supply and port symbols), so a symbol keeps its proportions
  at any zoom. Altium's "show name" and "show designator" flags on pins are kept,
  so a resistor's pins aren't labelled inside its body.
- **The page:** the sheet is drawn as a sheet of paper on a darker desk, with its
  border in from the edge and the grid on the paper. The paper size (A4, A3, A2,
  A1, A0, A, B, C, D, E, or a custom size read from a SchDoc) is in the *Sheet*
  card on the right when nothing is picked; it's read from and written to the
  SchDoc's sheet record, and the view fits to the page.
- **Select:** click or drag. Moving a part takes the wires on its pins with it,
  keeping them square. The right panel has the designator, value and footprint
  of what's picked (the symbol's footprints are offered), then the **checks**
  (pins joined to nothing, unnamed or repeated designators, parts with no
  footprint, labels touching nothing; pins named NC are left alone) and the
  **nets** worked out from the wiring.

Sheets save as Altium **`.SchDoc`** (File name ending `.SchDoc` in *Save as…*) or
as Coppersmith's own `.cssch`, and open from either, so a sheet from Altium or
Circuit Studio can be edited and sent back. The real test sheets read with the
same nets as the earlier reader found, and written sheets read back the same in
tests; **they haven't been opened in Altium or Circuit Studio.** Not yet: buses,
sheet entries and hierarchy, a sheet border and title block, text notes,
multi-select and copy/paste, moving the designator and value text, and
putting a schematic's changes onto the board (Update PCB).

### The 3D view

**View ▸ 3D view** (or **+ ▸ 3D view of the board**, or `coppersmith BOARD --3d`)
opens the board as a solid in a **3D** tab: green solder mask over the copper,
bare gold on the pads, white silkscreen, drilled and plated holes, the board's
own outline. Drag to turn it, right-drag or middle-drag to move, scroll or pinch
to zoom. The bar along the bottom has *Top*, *Bottom* and *Angle* views, *Fit*, a
switch for each of copper, mask and silk, six mask colours (green is the first)
and the board's thickness (1.6 mm to start). The picture is made from the board
as it is now, the way the Gerbers are (pads, tracks, vias, pours and fills for
copper; openings over the pads for the mask; the drawing and names for the
silkscreen), and is made again when the board changes while the tab is on show.
It is Copperview's 3D view (the same lighting and way of putting the layers on a
mesh of the board), taking its layers from Coppersmith's board instead of from
Gerber files.

Not yet: part bodies (the 3D view shows the board, not the components on it),
inner copper layers, and saving the picture.

### Altium and Circuit Studio boards and schematics

**Open** also takes `.PcbDoc` and `.CSPcbDoc` boards. The board comes in
unsaved, as a new Coppersmith board: outline, copper layers, parts (with
their footprints as placed), nets, tracks, arcs and vias, fills and solid copper regions, and the design rules (the
clearance rules, track widths, via sizes, mask and edge clearance).

Clearance comes in as Altium has it: a general gap, and a table of gaps for the
pairs of kinds of copper that need a different one (track, surface pad,
through-hole pad, via, region and pour). A pair the table doesn't list needs the
general gap, so on a board with a 3.5 mil general gap two tracks may be 3.5 mil
apart while a via and a track need 5 mil, and a pour keeps 18 mil from a track and
20 mil from a surface pad. The rule check, the router and the pours all ask for
the gap of the pair in question; the rules card on the right shows the table. A
clearance rule that applies only to particular objects (the fiducial pads, say)
isn't taken, and the status bar says so. A gap is allowed to come out 10 nm short:
Altium's coordinates are 2.5 nm apart, and each rounds on the way in.

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

### KiCad boards, sheets and libraries

**Open** (and `coppersmith FILE`) also takes KiCad 6 to 10's files:

- **`.kicad_pcb` boards**, with the rules from the `.kicad_pro` beside them
  (clearance, track width, via sizes, hole and ring sizes, edge clearance,
  hole to hole, and the net classes with the nets they hold, by name or
  pattern). Parts come in with their pads and silkscreen, courtyard and
  assembly drawing, a part underneath mirrored as KiCad has it; tracks (arcs
  included), vias, zones (poured again here, with their clearance, thermal gap
  and spoke width, island removal and fill order), keep-out zones with what each
  keeps out, and the edge from `Edge.Cuts`, however many lines and arcs it's
  drawn in. A pad on the far side of its part (a PCI edge connector's) makes a
  second part of the same name on that side. The board opens unsaved.
- **`.kicad_sch` sheets**: parts (many-unit symbols too) with their value and
  footprint, wires, net labels, global and hierarchical labels as ports,
  supplies as power ports in the right style, on a page of the sheet's own
  size. The sheet opens unsaved.
- **`.kicad_mod` footprints** (one, or a `.pretty` folder of them) and
  **`.kicad_sym` symbol libraries**, in the library editor and the Parts list.

What comes in is checked against KiCad's own demo boards: they read with every
part, track, via and zone, **nothing left unrouted, and no short circuits or
clearance errors**, and the PIC programmer's sheet comes in with every pin joined
but the six it flags no-connect. A gap is allowed to come out a micron short on a
KiCad board, as KiCad's own check allows. Left out, and said in the status bar:
text and drawings on the board itself, dimensions, cut-outs in the edge, hatched
zones (poured solid), trapezoid and custom-shaped pads (drawn as rectangles or
their anchor), blind and buried vias (made through vias), bus members and
hierarchical sheets on a sheet (a sub-sheet's own file opens by itself), and
alternate (De Morgan) symbol bodies. Older KiCad 5 files are only partly read.

**Out to Altium.** Footprints write as a `.PcbLib`, symbols as a `.SchLib` and
sheets as a `.SchDoc`; a board saves as a `.PcbDoc` (see Save as below, and
the Gerbers and drill files are still there too). Each of these is read back by
Coppersmith's own Altium reader and compared, but **none has been opened in
Altium itself**.

**Save as another format.** **File ▸ Save as…** asks for the format first:
Coppersmith's own `.cspcb` (everything kept), a KiCad 8-style `.kicad_pcb` with
the rules and net classes in a `.kicad_pro` beside it (an existing one is left
alone), or an Altium `.PcbDoc`. The name offered ends in `_CSMITH`, so saving a
board that came from another tool doesn't write over the original. The window lists what the chosen format would lose or change
before you pick a place. Exporting is a copy: the open board keeps its own
file and unsaved state. What is listed: pours come unfilled (press B in
KiCad), special clearances between kinds of copper, per-layer widths, matched
lengths, differential pairs and the other Altium rule kinds aren't carried,
rule on/off isn't, tracks on layers the board lacks, stack-up and outline.
Tracks, vias and pads translate exactly (checked on an Altium board: the same
connections are open before and after), and KiCad's demo boards survive the
trip. **The written file has been read back by Coppersmith, not opened in
KiCad itself.**

For an Altium `.PcbDoc` the same applies: the window lists the losses (pours
come as outlines to repour in Altium, power planes become ordinary pours, only
the general design rules and the class rules are written, parts carry no link to
a schematic or library, only designators are written as text, the layer stack
has default materials). Pads, tracks, arcs, vias, regions, keepouts, nets,
classes and pairs are written, and an Altium board's copper reads back exactly.
Its pad, via and text records start from records copied out of a real board.
**It has been read back by Coppersmith, and its record sizes match a real
board's, but it has not been opened in Altium, which may refuse or repair it.**

### PADS boards

**Open** (and `coppersmith FILE`) reads PADS Layout and PowerPCB boards in the
ASCII format (`.asc`, from *File ▸ Export* in PADS; the first line says
`!PADS-POWERPCB-…`, which is how it is told from LTspice's `.asc`). Versions 5
to 10 and the year-numbered ones are read, in the BASIC units PADS writes (and
mils, metric and inches). What comes in: the outline, parts with their pads and
silkscreen (a part underneath is turned and then mirrored, as PADS does), nets,
tracks, vias, pours (poured again here), free copper and keepouts, the layer
table, and the default clearance and width rules. PADS repeats shared stretches
of a track once for each connection it is in; they come in once.

Left out, and said in the status bar: free text, dimensions, board drawing that
isn't on a part, board cutouts, copper drawn in a footprint, blind and buried
vias (taken as through), slots (taken as round), and a pad that is a different
shape on the inner layers (it takes its outer shape on every layer: Coppersmith
has one shape a pad). The rules that are for particular nets are not read.

**Save as ▸ PADS ASCII board (.asc)** writes version 9.5. Every net is written
as pin-to-pin connections: copper that ends in the open is carried to the
nearest pin by an unrouted leg, as PADS does for a connection only partly
routed, so no copper is lost on a net with two pins or more (copper on a net of
fewer can't be written, and is counted). The window lists the rest: pours come
as outlines for PADS to pour, parts have one gate and no schematic link, only
the clearance table and widths are written as rules, arcs become short straight
tracks.

**Checked against six real boards** from KiCad's test set (2 to 8 copper
layers, versions 5 to 10, an EasyEDA export, one with every net unrouted): each
reads with the connections the file has, and writes and reads back with the same
parts, pads, nets, vias, open connections and rule findings. An Altium board
(EZDisplay) written as PADS has the same 675 connections open before and after.
**The file has not been opened in PADS itself**, and the format was worked out
from real files and KiCad's importer, not from Mentor's own description (see
`docs/PADS-FORMAT.md` for what it says and how sure each line is).

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
        kicad/         reading and writing KiCad boards, sheets, libraries
        pads/          reading and writing PADS ASCII boards
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
