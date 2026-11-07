# Where Coppersmith is going

The aim is a Circuit Studio–class tool: schematic capture, PCB layout and
library management in one program, in Copperview's look.

## 1. Layout core — done (0.1)

- Board model in millimetres, saved as JSON; parts carry their footprints.
- Footprint library: built-in package generators, user JSON files.
- Place, move (tracks follow), turn, flip to the bottom, copy, delete, box select.
- Interactive 45° routing on any copper layer, vias on the way, arrival on a pad ends the route, straight runs merge.
- Board outline drawing, 2/4/6 copper layers.
- Connectivity and the ratsnest (shortest tree per net), net highlighting.
- Live rule check: clearance and shorts, track width, hole size, annular ring, edge clearance, parts off the board, overlapping courtyards, unrouted connections.
- Gerber X2 + Excellon output in Altium naming; opens in Copperview.
- Undo/redo, save-changes guard, scripted screenshots for testing.

## 1½. Router, the Altium way — started (0.2)

Done: look-ahead segment, 45°/90°/any-angle corners with direction flip,
walk around / stop at first obstacle / ignore obstacles, Ctrl+click
auto-complete, vias mid-route on any layer, width sources, Backspace undoes a
click, loop and stub removal after re-routing, clearance shading, the routing
pill.

Done in 0.3: Push. Other nets' tracks bend aside as runs between their fixed
ends, cascading, springing back until clicked, with undo and Backspace.

Done in 0.3.1: via pushing (moves the least it can, clear on every layer,
its tracks on every layer follow), hug and push, pushed tracks re-bent only
where crowded, and a rule-check gate on everything the router places.

Done in 0.3.2: arc corners (45° and 90° with arc, radius keys), arcs as a
track shape through the board model, rule check, picking, editing and
Gerbers.

Done in 0.3.3: moving keeps corners — sliding a track keeps its and its
neighbours' directions, moving parts and vias brings tracks along at 45°,
corner drags re-lay in 45° legs; pushes are tested to stay at 45°.

Done in 0.3.4: sliding pushes what it runs into (cascading, vias too),
stops at what can't move, and is checked by the rule check on release.

Done in 0.3.5: every move pushes — parts, vias, tracks, corners, track ends,
turns and flips — with Ctrl to move freely.

Done in 0.3.6: the Shortest 45° rule — routes, moves and pushes never leave
square corners on 45° work.

Done in 0.3.7: arc corners stay tangent while sliding, and dragging an arc
changes its radius. 0.3.8: moving parts and vias keeps arc corners tangent.
0.3.9: pushing moves arcs too, keeping them tangent. 0.3.10: vias pushed on
one layer push on the others too.

Next:
- Gloss: re-smooth the routed and neighbouring tracks after each click.
- Interactive sliding of existing tracks, and dragging vias with the router.
- Follow mode, the length gauge, differential pairs, length tuning.

## 2. Library manager

Started in 0.3.13: Altium and Circuit Studio libraries are read — PcbLib
footprints into the library, SchLib symbols into a symbol model, IntLib
both. A library editor now reads and writes PcbLib and SchLib (footprint pads
and drawing, symbol pins and body); the written files read back identically in
Coppersmith, but **still need opening in Altium/Circuit Studio** to be sure
they accept them. Next for the editor: keepout, text and arc tools, multi-select,
IPC wizards, a components layer (symbol + footprint + parameters).

For schematic capture the pieces are in place: the symbol model, the SchLib
writer, and a SchDoc reader (parts, wires, junctions, net names, netlist
checked against a board). What's missing is a SchDoc writer and the sheet
editor itself.

0.3.14 reads boards (PcbDoc/CSPcbDoc: parts, nets, copper, rules) and
schematics (SchDoc, PrjPcb netlists checked against the board). 0.3.15
brings their pours, fills and plane layers in. Still to come from them:
pour cutouts, designator text positions, and schematic drawing once there's
a schematic editor.


- Footprint editor: pads (shape, size, drill, turn, numbering), pad arrays, silkscreen, courtyard and assembly drawing, IPC-7351 wizards for the package families already generated.
- Symbol editor: pins (number, name, type), body graphics, multi-part symbols.
- Components: symbol + footprint + pin map + parameters (value, manufacturer part number, supplier links) — the thing a schematic places.
- Libraries as files you can keep in git; search across them; usage report ("which boards use this").
- Import KiCad `.kicad_mod` and `.kicad_sym`, so the big public libraries are reachable on day one.

## 3. Schematic capture

Started: a schematic editor (File ▸ Schematic editor…) places parts from
symbol libraries (Altium SchLib/IntLib, plus a few built in), draws square
wires with automatic junction dots, net labels (numbers count up) and power
ports, drags parts with their wires following, turns and mirrors, and works out
the nets and a first set of checks as you go. Sheets open and save as Altium
`.SchDoc` (the real test sheets read with the same nets as before) or as a
Coppersmith `.cssch`. **Written SchDocs haven't been opened in Altium or Circuit
Studio.** Next: sheet frame and title block, designator/value text you can move,
buses and sheet entries, hierarchical sheets, annotation, a fuller ERC
(output-to-output, missing power), multi-select, copy and paste.

- Sheets, symbols, wires, junctions, net labels, power ports, buses, hierarchical sheets.
- Annotation, electrical rule check (unconnected pins, output-to-output, missing power).
- Netlist out; **Update PCB**: an engineering-change list (parts added, removed, renets) applied to the board, and back the other way for refdes changes.
- Cross-probing: pick in one, highlight in the other.

## 4. Copper pours

Done in 0.3.15 (on i_overlay): pours with their own clearance and edge gap,
solid or thermal-relief joins, island, small-piece and neck removal, pour
order, power planes (whole-board or split, cleared from holes), fills;
poured again after every change, checked, drawn, written to Gerbers.

- Still to do: editing a pour's corners in place, pour cutouts and keep-out areas, hatched pours, per-net clearance rules.

## 5. Routing

- Walk-around and push-and-shove, differential pairs, length tuning, teardrops.
- Net classes and scoped rules (power nets wider, high-voltage clearance).

## 6. Outputs

- Bill of materials, pick-and-place, fab and assembly drawings as PDF, one zip for the maker, IPC-2581.

## 7. 3D

- Started: a 3D tab of the bare board (green mask, gold pads, white silk, drilled holes), after Copperview's. Next: part bodies, STEP out, saving the picture.

## Under the hood, as boards grow

- Move drawing to Copperview's GPU layer path (gpu.rs) so thousand-part boards stay smooth.
- A spatial index for picking and the rule check, and incremental checks of only what changed.
