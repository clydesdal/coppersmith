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

- Footprint editor: pads (shape, size, drill, turn, numbering), pad arrays, silkscreen, courtyard and assembly drawing, IPC-7351 wizards for the package families already generated.
- Symbol editor: pins (number, name, type), body graphics, multi-part symbols.
- Components: symbol + footprint + pin map + parameters (value, manufacturer part number, supplier links) — the thing a schematic places.
- Libraries as files you can keep in git; search across them; usage report ("which boards use this").
- Import KiCad `.kicad_mod` and `.kicad_sym`, so the big public libraries are reachable on day one.

## 3. Schematic capture

- Sheets, symbols, wires, junctions, net labels, power ports, buses, hierarchical sheets.
- Annotation, electrical rule check (unconnected pins, output-to-output, missing power).
- Netlist out; **Update PCB**: an engineering-change list (parts added, removed, renets) applied to the board, and back the other way for refdes changes.
- Cross-probing: pick in one, highlight in the other.

## 4. Copper pours

- Polygons with clearance, thermal reliefs, island removal, priority — needs a polygon boolean engine (i_overlay or a Clipper2 port).
- Keep-out areas.

## 5. Routing

- Walk-around and push-and-shove, differential pairs, length tuning, teardrops.
- Net classes and scoped rules (power nets wider, high-voltage clearance).

## 6. Outputs

- Bill of materials, pick-and-place, fab and assembly drawings as PDF, one zip for the maker, IPC-2581.

## 7. 3D

- Reuse Copperview's 3D board view; part bodies; STEP out.

## Under the hood, as boards grow

- Move drawing to Copperview's GPU layer path (gpu.rs) so thousand-part boards stay smooth.
- A spatial index for picking and the rule check, and incremental checks of only what changed.
