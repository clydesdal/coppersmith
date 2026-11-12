# PADS Layout / PowerPCB ASCII board format (.asc): reader/writer reference

Sources: KiCad's ASCII importer (`pads_parser.cpp`, `pcb_io_pads.cpp`, `pads_pcb_converter.cpp`, `pads_layer_mapper.cpp`), used only as a reference for the format, plus six real files. Every claim marked **[verified]** was checked against the samples with small analysis scripts (not kept here). **[KiCad]** means the claim comes only from KiCad's code. **[unverified]** means neither source settles it.

| Sample | Header | Cu layers | Notes |
|---|---|---|---|
| ClaySight_MK1 | `!PADS-POWERPCB-V10.0-BASIC-250L!` | 2 | EasyEDA export, minimal, layer numbers +100 |
| MAIS_FC | `!PADS-POWERPCB-V5.0-BASIC!` | 2 | oldest dialect, no font lines |
| LCORE_2 | `!PADS-POWERPCB-V2005.0-BASIC!` | 2 | |
| Dexter_MotorCtrl | `!PADS-POWERPCB-V2007.0-BASIC!` | 8 | planes, pours, teardrops |
| Ems4_Rev2 | `!PADS-POWERPCB-V9.4-BASIC!` | 6 | planes, pours, numeric refdes "1","2" |
| TMS1mmX19 | `!PADS-POWERPCB-V9.5-BASIC!` | 2 | all nets unrouted |

All six samples use BASIC units. The MILS, METRIC and INCH modes come only from KiCad's code.

---------------------------------------------------------------------------------------------------
## 0. Lexical rules (all files)

- The format is line-oriented. Tokens are separated by one or more spaces. All samples use LF line endings and no tabs. Read CRLF anyway: trim `\r`, and trim leading and trailing whitespace from every line. Columns are padded with spaces (`CLOSED 5   952500 15  `), so never parse by column position.
- Read the file as bytes or Latin-1. Dexter's `*MISC*` CAM section contains raw bytes (`\xe4n\x12`), so it is not valid UTF-8.
- **Blank lines** can appear anywhere and carry no meaning. Skip them.
- **`*REMARK*` lines** are comments. The ones PADS writes contain the field grammar of the following section, so they make useful documentation. Skip them.
- A **section header** is a line whose first token is `*NAME*`. It may be followed by free text (`*PCB*        GENERAL PARAMETERS OF THE PCB DESIGN`), which you ignore. Inside `*ROUTE*`, `*SIGNAL*` starts a sub-record, not a new section.
- Records have no terminator. Their extent comes **only from the counts in their header lines** (pieces, corners, terminals, stacks, text, labels, stack lines). A reader is a count-driven state machine. Never use "next line starts with a letter" to find where a record ends: decal names, part-type names and refdes values can be numeric (Ems4 has parts named `1` and `2`).
- The file ends with `*END*`, optionally followed by text (`*END*     OF ASCII OUTPUT FILE`).
- The same section keyword can appear more than once. `*MISC*` appears five times in the PADS-native samples. Concatenate the occurrences.
- The order of sections in the PADS-native samples (V2005 and later) is: `*PCB* *REUSE* *TEXT* *LINES* [*CLUSTER*] *VIA* *PARTDECAL* *PARTTYPE* *PART* *ROUTE* [*POUR*] *TESTPOINT* *MISC*×5 *END*`. In ClaySight the order is `*PCB* *TEXT* *LINES* *POUR* *CLUSTER* *VIA* *PARTDECAL* *PARTTYPE* *PART* *ROUTE* *MISC* *END*`. KiCad accepts any order. A reader should also accept any order, except that resolving names (via name → `*VIA*`, part type → decal) must wait until the whole file has been read.

## 1. Header line, version dialects, units, axes and angles

### 1.1 Header line
Form: `!PADS-<PRODUCT>-<VERSION>-<UNITS>[-<MODE>]! <free text>`

Split the text between the two `!` characters on `-`:

| Position | Meaning | Values |
|---|---|---|
| 0 | literal | `PADS` |
| 1 | product | `POWERPCB` |
| 2 | version | `V5.0`, `V2005.0`, `V2007.0`, `V9.4`, `V9.5`, `V10.0` |
| 3 | units | `BASIC`, `MILS`, `METRIC` (KiCad also accepts `MIL`, `MM`, `INCH`, `INCHES`) |
| 4 | mode | ClaySight has `250L` (250-layer numbering mode, see §2.3) |

Real lines:
```
!PADS-POWERPCB-V9.5-BASIC! DESIGN DATABASE ASCII FILE 1.0      (TMS1mmX19)
!PADS-POWERPCB-V10.0-BASIC-250L! DESIGN DATABASE ASCII FILE 1.  (ClaySight)
!PADS-POWERPCB-V5.0-BASIC! DESIGN DATABASE ASCII FILE 1.0      (MAIS_FC)
```
KiCad recognises a board file by the substring `!PADS-` on the first line. Library files instead start with `*PADS-LIBRARY-...*`; this document does not cover them.

### 1.2 Version dialects [verified]
Let `major` be the integer between `V` and the first `.`. This gives 5, 2005, 2007, 9 or 10. Year-numbered releases come chronologically between V5 and V9. The grammar differs in these places:

| Feature | V5 | V2005 | V2007 | V9.x, V10 |
|---|---|---|---|---|
| Font line (`Regular <Romansim Stroke Font>`) after each text or label attribute line | no | yes | yes | yes |
| `LINESTYLE` field in piece lines (`*LINES*`, `*PARTDECAL*`) | no | no | no | yes |
| `CORNERRADIUS` field in pad and via rows of shape S, RF (and RC) | no | no | no | yes |
| Pin name as 5th field of decal `T` lines | no | no | yes | yes |
| `UNITS` field in the `*PARTTYPE*` header | yes | yes | no | no |
| `FLAGS` field in the `*LINES*` item header | no | no | yes | yes |
| `PRIORITY` in the `*POUR*` header grammar | no | yes | yes | yes |

Practical rules:
- `has_font_line = major >= 9` (this is true for 2005 and 2007).
- `has_linestyle = has_corner = (9 <= major < 2000)`.
- Each file's `*REMARK*` lines state its grammar, for example `PIECETYPE CORNERS WIDTHHGHT LINESTYLE LEVEL [RESTRICTIONS]` in V9 versus `PIECETYPE CORNERS WIDTHHGHT LEVEL RESTRICTIONS` in V2007.

KiCad applies the V9 piece, pad and LINES grammar to every version. As a result it mis-reads levels in V5, V2005 and V2007 `*LINES*` pieces, and S-pad drills in V2007. Do not copy that behaviour.

### 1.3 Units
- **BASIC** (all six samples): every length in the file is an integer in BASIC units. **1 mil = 38100 BASIC**, so **1 mm = 1 500 000 BASIC** exactly and 1 BASIC = 2/3 nm. To convert: `mm = v / 1_500_000`, `mils = v / 38100`.
  - Checks: ClaySight `LINES` width 381000 = 10 mil. `STANDARDVIA 381000` gives a drill of 10 mil. MAIS terminals are 750000 BASIC apart, which is 0.5 mm and matches the pitch of the Molex 54722 connector. Copper thickness 51435 = 1.35 mil, which is 1 oz copper.
  - In BASIC files, `*PCB* UNITS` (LCORE and MAIS have `UNITS 1`) and the decal unit letters are **display preferences only**. They do not change how numbers are scaled. KiCad's `decalUnitScale()` agrees: in BASIC mode it ignores the decal letter.
- **MILS, METRIC, INCHES** [KiCad, unverified]: the header token sets the default. A later `*PCB* UNITS n` line overrides it: `0` = mils, `1` = metric (mm), `2` = inches. Numbers may then be decimals. Conversion to mm: `v*0.0254` for mils, `v` for metric, `v*25.4` for inches. Decal coordinates use the decal's own letter: `I` (also `MIL`, `MILS`) = mils, `M` (also `MM`, `METRIC`) = mm. Part X/Y use the board units.
- **Values that are not lengths**, even in BASIC files:
  - Text height and stroke width are BASIC lengths.
  - `TEARDROP` route parameters (`P 90 90`) are percentages.
  - `*MISC* DFT_CONFIGURATION` has its own `UNITS MILS` and plain-mil numbers (`DRILL_SIZE 69`).
  - `DIELECTRIC` is a permittivity (`4.300000`).
  - Scale and flag fields have no units.
- Numbers can exceed ±6×10⁸ (TMS: `-671682376`). Parse every number as f64, or as i64 in BASIC.

### 1.4 Axes, origin and angles
- **The Y axis points up** (mathematical convention). Angles are measured counter-clockwise from +X. To convert to a Y-down system such as KiCad, negate Y, which also reverses the winding direction of arcs.
- **`ORIGIN x y`** (in `*PCB*`) gives the user origin's location in PADS's internal coordinate frame.
  - Coordinates in the rest of the file are used exactly as written. KiCad does the same and then re-centres on the board-outline bounding box.
  - Hypothesis [unverified]: absolute database coordinate = ORIGIN + file coordinate. In all six samples every such sum is positive. For example, TMS has `ORIGIN 1066800000 1066800000` and its most negative item is at `-671682376`.
- **Coordinate frames:**

| Frame | Where |
|---|---|
| Absolute | `*TEXT*`, `*PART*` X/Y, `*ROUTE*` points |
| Relative to the item's XLOC/YLOC | `*LINES*` and `*POUR*` piece points, arc boxes, and text inside LINES items |
| Relative to the part origin | decal pieces, terminals, labels; part label X/Y |

- **Angle encodings:**

| Where | Units | Example |
|---|---|---|
| Part ORI, text ORI, label ORI, pad FINORI, slot orientation, thermal spoke orientation | float degrees, 0–360, CCW | `270.000`, `90.000`; ClaySight writes the integer `0` |
| Arc fields in `*LINES*` and `*PARTDECAL*` points, `*POUR*` arc lines | integer **tenths of a degree**; positive = CCW, negative = CW | `1800 -900` = start at 180°, sweep 90° clockwise |

## 2. `*PCB*` section, layer count, layer numbering

### 2.1 Keywords
Each line has the form `KEYWORD value [value] [free comment]`. Only the first one or two tokens are data. Real lines (Dexter):
```
UNITS        0              2=Inches 1=Metric 0=Mils
MAXIMUMLAYER 8              Maximum routing layer
ORIGIN       771525000 963930000  User defined origin location
VIAPSHVIA    STANDARDVIA
```

| Keyword | Meaning |
|---|---|
| `UNITS` | see §1.3 |
| `MAXIMUMLAYER N` | **copper layer count**; copper layers are numbered 1..N. ClaySight, LCORE, MAIS, TMS: 2; Ems4: 6; Dexter: 8 [verified against LAYER DATA] |
| `ORIGIN x y` | user origin |
| `USERGRID` | grid |
| `LINEWIDTH` | default line width |
| `TEXTSIZE h w` | default text height and stroke |
| `DRLOVERSIZE` | plated-hole drill oversize |
| `TEARDROP`, `THERFLAGS`, `PLANEFLAGS`, `ROUTERFLAGS` | opaque bit masks |
| `VIAPSHVIA name` | via used for via patterns. KiCad uses it as the default signal via. |
| `MINCLEAR DEFAULTCLEAR MINTRACKWID DEFAULTTRACKWID MINVIASIZE DEFAULTVIASIZE MINVIADRILL DEFAULTVIADRILL HOLEHOLE SILKCLEAR MASKCLEAR` | [KiCad] design-rule keywords. They appear in **none** of the samples; real rules live in `*MISC*` (§11). |

Everything else (JOBTIME, SCALE, WINDOWCENTER, colours, …) can be ignored.

ClaySight's `*PCB*` is minimal: `UNITS 0`, `MAXIMUMLAYER 2`, `THERFLAGS 1024`, `TEARDROP 2320474`, `ORIGIN 381000000 381000000`.

### 2.2 Layer numbers
- **Copper:** 1 = top, N = bottom, 2..N−1 = inner, where N = MAXIMUMLAYER. In LAYER DATA, layers N+1..20 (or 30) exist as `UNASSIGNED` placeholders.
- **0** means different things by context:
  - "All layers" for board-outline pieces, keepouts and decal 2D pieces in old dialects (`CLOSED 7 381000 0 0`).
  - "Unrouted" in `*ROUTE*` (§7).
  - "Bottom/opposite side" in pad and via stack rows (§4.5).
- **−2 and −1** appear only in pad and via stack rows (§4.5).
- **65** appears only in `*ROUTE*` and means "end of connection" (§7).
- **Non-copper layers are defined per file. Never hard-code them.** Read `*MISC*` → `LAYER DATA` (§11.1) and classify each layer by `LAYER_TYPE` plus whether its name contains top or bottom. Two numbering schemes appear in the samples [verified]:

| Function | Dexter, MAIS, TMS | Ems4, LCORE | ClaySight (250L) |
|---|---|---|---|
| Solder mask top / bottom | 21 / 28 | 23 / 26 | 121 / 128 |
| Paste top / bottom | 23 / 22 | 24 / 27 | 123 / 122 |
| Silkscreen top / bottom | 26 / 29 | 22 / 25 | 126 / 129 |
| Assembly top / bottom | 27 / 30 | (30 = "Size Drawing") | 127 / 130 |
| Drill drawing | 24 | 29 | 125 ("Document Layer") |

  So the "odd 126" seen in ClaySight piece and text lines is **Top Silkscreen in 250-layer numbering**: the classic 1–30 scheme with documentation layers shifted by +100. KiCad's built-in fallback (25 = mask top, 26 = silk top, 27 = silk bottom, 28 = mask bottom) is only used when LAYER DATA is absent, and it is **wrong** for every sample. A writer must therefore emit LAYER DATA.
- **2D items on a copper layer number inside decals and labels** (labels on level 1 are common) are documentation items, not copper. PADS draws them on the side's silkscreen or assembly layer, and KiCad maps them to silkscreen. The exception is `COP*` pieces (§4.2).

## 3. `*LINES*`: 2D lines, board outline, copper, keepouts, dimensions

### 3.1 Item header
- V2007 and later: `NAME TYPE XLOC YLOC PIECES FLAGS [TEXTCOUNT [SIGNAME]]`
- V5 and V2005: `NAME TYPE XLOC YLOC PIECES [TEXTCOUNT [SIGNAME]]`

The REMARK lines claim `NAME TYPE XLOC YLOC PIECES TEXT SIGSTR`, but V2007+ files have the extra FLAGS field. In every sample FLAGS is 0, except Ems4 COPPER items, which have 2.

TYPE values:

| TYPE | Meaning | Samples |
|---|---|---|
| `LINES` | 2D graphics | all |
| `BOARD` | **board outline**: exactly one item per file | all |
| `COPPER` | free copper; SIGNAME, if present, names the net | Ems4, LCORE |
| `COPCUT` | copper cutout | TMS |
| `KEEPOUT` | keepout | Dexter, LCORE, TMS |

KiCad also knows `RESTRICTVIA`, `RESTRICTROUTE`, `RESTRICTAREA`, `PLACEMENT_KEEPOUT` [KiCad]. A `LINES` item with several pieces of type BASPNT, ARWLN*, ARWHD* and EXTLN* is a **dimension**; PADS names these items `DIM…`. An optional `.REUSE. instance rsignal` line may follow the header.

Real lines:
```
DRW14925258      BOARD    -3810000 228600000 1   0      (Dexter, V2007: FLAGS present)
DRW27860889      BOARD    15625000 -36324500 1           (LCORE, V2005: no FLAGS)
DIM7579012       LINES    0      247650000 8   0 1       (Dexter: 8 pieces, 1 text)
line1 BOARD 0 0 9 0                                      (ClaySight)
```
**To find the board outline**, use the item whose TYPE is `BOARD`. Its `CLOSED` or `BRDCLS` pieces are the outline, and its `BRDCIR` or `BRDCLS` pieces after the first are cutouts or holes. ClaySight has 1 CLOSED piece plus 8 BRDCIR mounting holes. A `LINES` item that happens to sit on a "board" layer is not the outline.

### 3.2 Piece line
- V9+: `PIECETYPE CORNERS WIDTH LINESTYLE LEVEL [RESTRICTIONS | PINNUM]`
- V5, V2005, V2007: `PIECETYPE CORNERS WIDTH LEVEL [RESTRICTIONS]`

WIDTH is the line width in length units. LINESTYLE is 0 in all samples. LEVEL is the layer (§2.2). Real lines:
```
CLOSED 7 381000 0 0          (ClaySight V10: linestyle 0, level 0)
OPEN   217 152400 0   22     (Ems4 V9.4: level 22 = Silkscreen Top in that file)
CLOSED 5   952500 15         (Dexter V2007: level 15)
KPTCLS 13  0   0   1   C     (TMS V9.5 keepout: level 1, restrictions "C")
KPTCIR 2   0   2   RC        (LCORE V2005: level 2, restrictions "RC")
```

PIECETYPE values:

| Group | Types | Notes |
|---|---|---|
| Lines | `OPEN` (polyline), `CLOSED` (closed polygon; the last point repeats the first) | |
| Circle | `CIRCLE` | 2 corners = opposite ends of a diameter (in all samples, horizontal) |
| Copper | `COPOPN`, `COPCLS` (filled), `COPCIR` (filled circle), `COPCUT`, `COPCCO` (cutouts) | |
| Board | `BRDCLS`, `BRDCIR` | board cutouts |
| Keepout | `KPTCLS`, `KPTCIR` | RESTRICTIONS letters: P = placement, H = height (WIDTH holds the height), R = trace, C = copper, V = via/jumper, T = test point, A = accordion [KiCad] |
| Dimension | `BASPNT`, `ARWLN1/2`, `ARWHD1/2`, `EXTLN1/2` | The 5th field of a dimension piece is a dimension flag (`2048`, `4096`), not RESTRICTIONS. |
| Decal group | `TAG` | see §4.2 |

### 3.3 Point lines and arcs [verified]
There are exactly CORNERS lines after the piece line. Each is either
- `x y`, a plain vertex, or
- `x y AB AA X1 Y1 X2 Y2`, an **arc vertex**.

For an arc vertex:
- (x, y) is a normal vertex **and is the start of an arc**.
- AB is the begin angle and AA the sweep, both in tenths of a degree (positive = CCW).
- (X1, Y1)-(X2, Y2) is the bounding box of the arc's **full circle**: centre = box midpoint, radius = (X2−X1)/2.
- The arc ends at the **next** vertex line. The arc line counts as one of the CORNERS.

TMS decal example:
```
CLOSED 9   374904 0   27
-15937500 3135000 1800 -900 -15937500 2535000 -14737500 3735000
-15337500 3735000
```
Centre (−15337500, 3135000), r = 600000. The arc starts at 180°, at the given point, sweeps −90° clockwise and ends at 90°, which is the next vertex (−15337500, 3735000). The LCORE board outline mixes 15 such arc corners.

Rounding: start = centre + r·∠AB holds only to about 0.1° (~57k BASIC off on a 33M radius in LCORE). **Use the explicit (x, y) as the start and the next vertex as the end.** Use the box centre for the centre and AA's sign for the direction. KiCad instead recomputes both endpoints from the angles. When writing an arc, emit the start vertex with the 6 extra fields, then the end vertex.

### 3.4 Text inside LINES items
After the pieces come TEXTCOUNT text records in the `*TEXT*` layout (§10), with coordinates relative to XLOC/YLOC.

## 4. `*PARTDECAL*`: footprints

### 4.1 Header (9 fields)
`NAME UNITS ORIX ORIY PIECES TERMINALS STACKS TEXT LABELS`

| Field | Meaning |
|---|---|
| NAME | decal name (may contain `/`, start with a digit, etc.) |
| UNITS | `I` (imperial) or `M` (metric). Display units only in BASIC files (§1.3); KiCad also accepts `MM` and `D`. |
| ORIX ORIY | decal origin location from the decal editor. Ignored by KiCad, and nothing in the file depends on it. Write `0 0`. |
| PIECES | count of 2D or copper piece records (each followed by its points) |
| TERMINALS | count of `T` lines |
| STACKS | count of `PAD` records |
| TEXT | count of free text records |
| LABELS | count of attribute labels |

Record order: pieces, then TEXT texts, then LABELS labels, then terminals, then stacks. Real lines:
```
LSHM-130-XX.X-XX-DV-A-S M 6375001 -2775000  2 64 5 2 2   (TMS)
DIODE-1N4001 I 0 0 11 2 1 0 2                            (ClaySight)
54722-0201       M 49320000 48288750  5 20 1 0 2          (MAIS V5)
```

### 4.2 Pieces
These use the same grammar as §3.2/§3.3. Decal-specific details:
- `COPCLS`, `COPOPN`, `COPCIR`, `COPCUT`, `COPCCO` are real **copper** on LEVEL. The optional last field PINNUM is the **0-based terminal index** of the pin the copper belongs to [verified: 224 of 224 COPCLS polygons in TMS and Ems4 contain terminal[PINNUM]]. Example (TMS): `COPCLS 5   15000 0   1    0`.
- `TAG CORNERS WIDTH LINESTYLE LEVEL`, with 0 corners: LEVEL 1 opens a group of pieces and LEVEL 0 closes it [TMS; meaning taken from KiCad].
- `KPTCLS` and `KPTCIR` are keepouts with RESTRICTIONS. `BRDCIR` (seen in ClaySight decals) is a board cutout carried by the part.
- Level 0 in pre-V9 decals (`OPEN 2 152400 0`, LCORE) means "all layers". KiCad puts these items on top silkscreen.

### 4.3 Text and labels
TEXT records come first. Each is an attribute line `X Y ORI LEVEL HEIGHT WIDTH MIRROR HJUST VJUST [NDIM]` (no keyword), then the font line (V2005+), then the content line.

Each of the LABELS records is:
1. `VISIBLE X Y ORI LEVEL HEIGHT WIDTH MIRROR HJUST VJUST [RIGHTREADING]`
2. the font line (V2005+)
3. a line with the **label name**

| Field | Values |
|---|---|
| VISIBLE | `VALUE` (show the value; seen), `NONE` (hidden; seen); KiCad also knows `NAME`, `FULL_NAME`, `BOTH`, `FULL_BOTH` |
| Label name | `Ref.Des.` → reference designator, `Part Type` → part-type name; any other text names an attribute whose value is in `*MISC* ATTRIBUTE VALUES` |
| MIRROR | `N` or `M` |
| HJUST | `LEFT`, `CENTER`, `RIGHT` |
| VJUST | `UP`, `CENTER`, `DOWN` |
| RIGHTREADING | `ORTHO` and others |

TMS:
```
   11250000    -6000000   0.000 26     3810000      381000 N CENTER CENTER 0    <- TEXT (content "1")
VALUE           0     7500000   0.000  1     3810000      381000 N CENTER CENTER
Regular <Romansim Stroke Font>
Ref.Des.
```
KiCad parses TEXT records as if they were labels and drops their first field. For KiCad compatibility, write TEXT = 0.

### 4.4 Terminals
`T<X> Y NMX NMY [PINNAME]`. The `T` is glued to X (`T-7620038`), so strip the first character.
- NMX, NMY: position of the pin-number text.
- PINNAME: the pin name used in nets (`1`, `A`, `G1`, `MH1`, `PAD`, `J3`).
- V5 and V2005 have no PINNAME; the pins are then named `1`..n in line order [verified: 4 fields in MAIS and LCORE, 5 elsewhere].

Real lines:
```
T-7620038 38 -7620038 38 A              (ClaySight)
T12750000 -1275000 12750000 -1275000 MH1   (TMS)
T-3375000 -4125000 -3375000 -4125000    (MAIS V5)
```

### 4.5 Pad stacks
Header: `PAD <n> <ROWS> [P|N | drill [slot_ori slot_len slot_off]]`. The bracketed extras appear only in KiCad's code; every sample header has exactly 3 tokens.
- `n = 0` is the default stack for every terminal.
- `n = k ≥ 1` overrides the stack for the **k-th T line (1-based)**. In TMS, `PAD 61..64` cover terminals G1, G2, MH1 and MH2.

Each of the ROWS lines has the form `LEVEL SIZE SHAPE <shape fields> [DRILL [PLATED [SLOT_ORI SLOT_LEN SLOT_OFF]]]`.

**LEVEL**

| Value | Meaning |
|---|---|
| −2 | mounted (component) side |
| −1 | inner layers |
| 0 | opposite side |
| k > 0 | that specific layer (mask, paste or assembly, from the file's layer table) |

Each stack has one −2 row, one −1 row and one 0 row, in that order, followed optionally by extra rows. A size of 0 means no pad on that layer.

**Disagreement:** KiCad treats −1 as bottom and 0 as inner. The samples support top/inner/bottom:
- Plane thermal (RT) and antipad (RA) rows are always on −1. Planes are inner layers.
- Dexter `PAD 1 4: -2 S | -1 R | 0 S | 25 R` describes a square pin 1 on both outer sides and round on the inner layers. That only makes sense if 0 is the bottom.

For a writer, the ambiguity is harmless if the −1 and 0 rows use the same size for through-hole pads, and size 0 for SMD pads.

**Shape fields** (V9+ dialect; drop `CORNER` for V5, V2005 and V2007):

| SHAPE | Rest of row | Meaning |
|---|---|---|
| `R` | `[DRILL [P|N]]` | round, diameter SIZE |
| `S` | `CORNER [DRILL [P|N]]` | square, side SIZE. CORNER > 0 rounds the corners, < 0 chamfers them, 0 leaves them square. |
| `RF` | `FINORI FINLEN FINOFF CORNER [DRILL [P|N]]` | rectangular finger: width SIZE, length FINLEN along an axis rotated FINORI degrees, offset FINOFF along that axis |
| `OF` | `FINORI FINLEN FINOFF [DRILL [P|N [SLOT_ORI SLOT_LEN SLOT_OFF]]]` | oval (obround) finger; same geometry, no CORNER |
| `A` | `INNER_DIA` | annular |
| `RT`, `ST` | `ORI OUTER_DIA SPOKE_W SPOKE_N` | round or square plane thermal; SIZE = inner diameter |
| `RA`, `SA` | none | plane antipad, diameter or side SIZE |
| `O`, `OC`, `RC` | | [KiCad only] odd shape, odd circle, rectangle with corner |

**Drill and plating:**
- The drill appears only on the **first (−2) row**. A non-zero DRILL means through-hole.
- PLATED is `P` or `N`. If it is missing and DRILL > 0, the hole is plated.
- An SMD pad has −2 with DRILL 0 or no drill, and −1 and 0 rows of size 0.
- A non-plated mounting hole has size 0, drill > 0 and `N` (TMS `-2 0 R 2250000 N`).
- A slot comes after `P`/`N`: `ORI LEN OFF`.

Real stacks:
```
PAD 0 3                                            (TMS, SMD finger, V9.5)
-2 450000 RF  90.000 2250000 0   0   0 N
-1 0   S   0
0  0   S   0
PAD 0 7                                            (TMS, plated TH, 1425000 drill, plane thermal + antipad)
-2 2475000 R   1425000
-1 2475000 R
-1 2325000 RT  45.000 2925000 375000 4
-1 2925000 RA
0  2475000 R
21 2475000 R
28 2475000 R
-2 2971800 OF 0 2971800 0 1499616 P 0 1499997 0    (ClaySight: oval TH pad with slot)
-2 3600000 S 2100000                               (Dexter V2007: square pad, drill 2100000, no CORNER field)
```

### 4.6 Alternate decals
A part type's DECALNM field can be a colon-separated list (`LED:LED-S:0805:LEDC2013X120M`, TMS). The part's ALT field (§5.2) is a **0-based index** into that list. TMS `D1 LED … 0.000 U N 3 …` uses `LEDC2013X120M`, and LCORE `U3 … ALT 1` uses `LP5900SD33`; both are defined decals [verified]. KiCad additionally accepts `PARTTYPE@DECAL` in the part line.

## 5. `*PARTTYPE*` and `*PART*`

### 5.1 `*PARTTYPE*`
Header:
- V2007 and later: `NAME DECALNM TYPE GATES SIGPINS UNUSEDPINNMS FLAGS ECO`
- V5 and V2005: `NAME DECALNM UNITS TYPE GATES SIGPINS PINNMS FLAGS ECO`

TYPE is a free string (`CON RES CAP UND DIO QFN …`). Following the header:
- **GATES** gate records. Each is `G <swaptype> <npins>` followed by npins pin tokens `PIN.SWAPTYPE.PINTYPE[.FUNCNAME]`, wrapped six per line. Example: `1.0.U.`; PINTYPE is one of `U S B C T L Z P G`.
- **SIGPINS** lines `SIGPIN PIN [WIDTH] SIGNAME`.
- **UNUSEDPINNMS** pin-name tokens.
- An optional `{ attr value … }` block [KiCad].

Records have no terminator, so use the counts.

Real lines:
```
RES0603 0603 RES  1   0   0     0 Y          (TMS)
G 0 2
1.1.U. 2.1.U.
54722-0201 54722-0201 I CON  1   0   0     1 Y   (MAIS V5, UNITS present)
DIODE-1N4001 DIODE-1N4001 UND 0 0 0 0 Y          (ClaySight: zero gates, no pin list; KiCad accepts it)
```

### 5.2 `*PART*`
Header: `REFNM PTYPENM X Y ORI GLUE MIRROR ALT CLSTID CLSTATTR BROTHERID LABELS`

| Field | Meaning |
|---|---|
| REFNM | reference designator (may be numeric) |
| PTYPENM | part-type name, resolved through `*PARTTYPE*` to the decal list; KiCad falls back to treating it as a decal name |
| X Y | absolute position of the decal origin |
| ORI | rotation in degrees, CCW |
| GLUE | `U` (unglued) or `G` (glued) |
| MIRROR | **`N` = top side, `M` = bottom side (mirrored)** |
| ALT | 0-based alternate-decal index |
| CLSTID | −1 |
| CLSTATTR | 0 |
| BROTHERID | −1 |
| LABELS | number of label records that follow (same 3- or 2-line layout as §4.3). These are the instance's label positions, relative to the part origin. Text LEVELs are written for the top side and are not swapped for `M` parts. |

An optional `.REUSE. INSTANCE RPART` line may follow the header. KiCad expands `C{2-20}` refdes shortcuts [KiCad].

Real lines:
```
JM3             LSHM-130-XX.X-XX-DV-A-S 146984466 133350000 270.000 G M 0 -1 0 -1 2   (TMS, bottom)
U1 Raspberry_Pi_Pico_W 70484848 131444733 0 U N 0 -1 0 -1 15                      (ClaySight)
1               CC3B 7810500 6477000 0.000 U N 0 -1 0 -1 2                         (Ems4: refdes "1")
```

**Placement transform** [verified with 4,700 route endpoints that land on pins in 6 files]:
- Top (`N`): `P = (X, Y) + R(ORI)·(x, y)`
- Bottom (`M`): `P = (X, Y) + F·R(ORI)·(x, y)`, where F = (x → −x). The decal is rotated CCW by ORI first and then mirrored about the vertical axis through the origin. This is equivalent to mirroring first and then rotating by −ORI.

For bottom parts, pad row −2 lands on copper layer N and 0 lands on layer 1. Graphics and labels move to the paired bottom layer, as given by the `ASSOCIATED_*` names in LAYER DATA (§11.1).

## 6. `*NET*`
None of the six board files has a `*NET*` section. **Connectivity in a board .asc lives entirely in `*ROUTE*`** (§7).

`*NET*` belongs to PADS netlist files: `*SIGNAL* NAME` followed by lines of `REF.PIN` tokens, where `REF.PIN` is split at the first `.`. KiCad's `*NET*` code expects a bare `SIGNAL` keyword and also expands range shortcuts:

| Shortcut | Expands to |
|---|---|
| `U{4-8}.{7-8}` | the cartesian product of both ranges |
| `PRE{a-b}` | `PREa` … `PREb` |

Treat the `*NET*` grammar as [KiCad, unverified]. A writer should not emit `*NET*` in a board file.

## 7. `*ROUTE*`: nets, connections, tracks, vias

Structure:
```
*ROUTE*
*SIGNAL* <NETNAME> <SIGFLAG> <COLOR> [; comment]
<REF.PIN> <REF.PIN>            <- one connection (from-pin, to-pin)
<point line>                   <- its geometry, 2 or more points
...
<REF.PIN> <REF.PIN>
<point line> ...
```

**`*SIGNAL*` header:**
- SIGFLAG is an opaque bit mask: 0, 2 or 1026 for power nets in Ems4, 536870912, 2147483648, 2147484674.
- COLOR is −2 (default) or a colour index.
- V9.4+ append `; (All)`. Ignore everything after `;`. KiCad also uses any token that names a `*VIA*` as the net's default via.

**The net's pins** are the union of all pin tokens in its connection lines. Connections form a spanning set: TMS net REF2 has 6 connections over 7 pins. A one-pin net cannot be expressed.

**Recognising lines:** a connection line has 2 tokens that each contain `.` (plus optional `.REUSE. inst rsig` tails). A point line has at least 5 tokens whose first 5 are numeric. Do not test "first character is a digit": Ems4 has `2.1                             4.1`, which KiCad mis-reads.

**Point line:** `X Y LAYER WIDTH FLAGS [tokens…]`. The **LAYER and WIDTH on point i describe the segment from point i to point i+1** [verified: a via point carries the new layer, e.g. Ems4 `… 1 …` then `-15430500 419100 4 152400 256 STANDARDVIA …`].

LAYER values:

| LAYER | Meaning |
|---|---|
| 1..N | routed copper segment on that layer |
| 0 | **unrouted** (ratsnest) segment to the next point. No copper; it typically leaves a pin or passes through a plane. |
| 65 | **last point** of the connection; no segment follows. Every connection ends with 65 in all samples, including the 2-layer and 250L files. |

The first point lies on the from-pin and the last (65) on the to-pin. A fully unrouted connection is exactly 2 points (TMS):
```
*SIGNAL* NET-(R5-PAD1) 0 -2 ; (All)
U8.3                            R5.1
254179800 124023300 0 381000 1792  THERMAL
293789100 141541500 65 381000 1792  THERMAL
```

**Trailing tokens**, in this order (from the file's own REMARK):
1. `[ARCDIR | VIANAME]`
2. `THERMAL`
3. `[TEARDROP [P wid len [flags]] [N wid len [flags]]]`
4. `[JMPNAME S|E]`
5. `[.REUSE. inst rsig]`

| Token | Meaning |
|---|---|
| VIANAME | **a via at this point**, named as in `*VIA*`. Seen: `STANDARDVIA`, `VIAS`, `MICROVIA`. It can also appear on a layer-0 point, which gives a via into a plane (Ems4 `17526000 13525500 0 571500 67328 VIAS THERMAL`). |
| THERMAL | the point touches a pad or via. Seen on every pin end and every via point. |
| TEARDROP | teardrop on the previous (P) and/or next (N) segment side. The first point of a connection only carries N and the last only P [verified]. The values `90 90` are percentages. |
| CW, CCW | [KiCad, no sample has one] this point is the **centre** of an arc that runs from the previous point to the next point. The centre is not itself a vertex. |
| POWER | [KiCad] plane connection, often doubling as a via name |
| `<name> S`, `<name> E` | jumper start or end |

**FLAGS bits** (empirical, Dexter and Ems4):

| Bit | Meaning |
|---|---|
| 0x100 | set exactly when `THERMAL` is present |
| 0x200 | set when there is no N teardrop |
| 0x400 | set when there is no P teardrop |
| 0x800, 0x10000 | unknown; 0x10000 seen only on `VIAS` points |

So 1536 (0x600) is a plain corner and 1792 (0x700) is a pin or via corner without teardrops. ClaySight writes only those two values.

**Turning connections into tracks and vias** [verified behaviour]:
1. For each consecutive pair (p_i, p_i+1) with LAYER_i in 1..N, emit a segment on LAYER_i with WIDTH_i. Skip pairs where LAYER_i is 0 or 65.
2. Emit a via at every point with a VIANAME. Its padstack and span come from `*VIA*` (§8).
3. **De-duplicate.** PADS repeats shared trunks once per connection, so T-junctions appear as duplicated segments. Duplicate segments found: Dexter 542 of 2,952, Ems4 1,523 of 6,031, ClaySight 63 of 247. De-duplicate segments by (net, layer, unordered endpoints) and vias by (net, x, y).
4. ClaySight (EasyEDA) changes layer with an unrouted zero-length hop and no via token (`149351619 195497577 0 …` then the same XY on layer 1), and its `*VIA*` section is empty. Its vias are therefore implicit. KiCad inserts a via when the layer changes at the same XY.

A routed example (ClaySight), where the first point is on layer 0 (a tiny unrouted stub off the pad), then copper on layer 2:
```
U1.38 U30.5
99569778 198269733 0 38100 1792 THERMAL
99569778 198269352 2 1905000 1536
...
150494848 129539581 65 1905000 1792 THERMAL
```

## 8. `*VIA*`

Header: `NAME DRILL STACKLINES [DRILL_START DRILL_END]`. DRILL is the finished hole diameter. DRILL_START and DRILL_END (blind or buried span as copper layer numbers) never appear in the samples.

Rows: `LEVEL SIZE SHAPE …`. LEVEL is −2, −1, 0 or k, with the same meaning as for pad rows (§4.5). Shapes and fields are as in §4.5, with one **exception [verified, TMS]**: via thermal rows are written `LEVEL SIZE RT OUTER_DIA ORI SPOKE_W SPOKE_N`. For example, `-1 1524000 RT 2286000  45 571500  4` has the orientation *after* the outer diameter, whereas the decal form is `RT 45.000 2925000 375000 4`. KiCad reads both forms with the decal order.

A via whose rows are −2, −1 and 0 is a through via: annular ring = the −2 SIZE, span = 1..N. Its name is what route points reference.

Real lines:
```
STANDARDVIA      381000 3        (Dexter: 10 mil drill, 20 mil pad)
-2 762000 R
-1 762000 R
0  762000 R
VIAS             304800 3        (Ems4 V9.4: square via, CORNER 0)
-2 609600 S 0
```
All PADS-native samples define `JMPVIA_AAAAA`, which is used by jumpers. ClaySight has an empty `*VIA*`.

## 9. `*POUR*` and planes

Header: `NAME TYPE XLOC YLOC PIECES FLAGS [OWNER SIGNAME [HATCHGRID HATCHRAD [PRIORITY]]]`

| TYPE | Meaning | FLAGS / fields seen |
|---|---|---|
| `POUROUT` | user-drawn pour outline; **this is the zone** | OWNER = its own name; SIGNAME = net |
| `HATOUT` | computed fill outline | OWNER = the POUROUT; FLAGS 8 |
| `VOIDOUT` | hole in a HATOUT | OWNER = the HATOUT; FLAGS 128 |
| `PADTHERM`, `VIATHERM` | thermal spoke segments | SIGNAME `0`. The values `-1` and `0` mean "no net". |

PRIORITY: a lower number fills first, i.e. has higher priority. HATCHGRID −1 means default.

Piece line: `PIECETYPE CORNERS ARCS WIDTH LEVEL`, where PIECETYPE is `POLY`, `SEG`, `CIRCLE`, `CUTOUT`, `CIRCUT` or `POCUT`. There are **CORNERS + ARCS** point lines. Coordinates are relative to XLOC/YLOC.

**Pour arcs use a different encoding from §3.3** [verified]:
- An arc is a 4-field line `CX CY AB AA` (centre plus begin angle and sweep, in tenths of a degree), placed **between** its start vertex (the previous line) and its end vertex (the next line).
- `CIRCLE` has 2 diameter-end points.

Ems4 example: `-89602818 -362506580` / `-90106500 -362331000 3407 -515` / `-89930920 -362834682`. Here 340.7° from the centre gives the previous point, and sweeping −51.5° gives the next.

Real lines:
```
POR7579012 POUROUT -14668500 11239500 1 0 POR7579012 AGND -1 0.000000 1   (Ems4)
POLY 15 0 152400 2
ANP000000 HATOUT 76200000 381000000 1 8 POR7579012
ANP000007 VIATHERM 76200000 381000000 4 0 POR7579012 0 0
```
For a KiCad-style zone, take the POUROUT polygon, net, layer and priority. HATOUT minus VOIDOUT gives the last computed fill. VIATHERM and PADTHERM can be ignored.

**Planes** are declared per copper layer in `*MISC* LAYER DATA`:
- `PLANE NONE` or `PLANE MIXED` [seen]; KiCad's sources and PADS documentation also mention `CAM` (negative plane) [unverified].
- One `NET <name>` line per plane net (Ems4 layer 5 has five).
- `LAYER_TYPE` stays `ROUTING` even for plane layers (Dexter `Power P3_3v`).
- A MIXED, or split, plane's copper is the set of `*POUR*` items on that layer.

## 10. `*TEXT*` (free text)
Each record has 3 lines (2 in V5):
1. `X Y ORI LEVEL HEIGHT WIDTH MIRROR HJUST VJUST [NDIM] [.REUSE. INST]`
2. the font line (V2005+): `FONTSTYLE[:H[:D]] FONTFACE`
3. the content line

X and Y are absolute. HEIGHT is the character height and WIDTH the stroke width; both are lengths. MIRROR `M` means mirrored (bottom-side text). The trailing NDIM is `0` in every sample.

Font line forms:
- `Regular <Romansim Stroke Font>`: the built-in stroke font.
- `Regular,H=130,D=33 Lucida Handwriting`: a TrueType font with cell metrics.

The content runs to the end of the line. KiCad turns a literal `\n` into a newline. For `250L` (EasyEDA) files it also turns `_` into newline, which is a guess: ClaySight has `CLAYSIGHT MCU V.1_The Ohio State University_PCB…`.

Real lines:
```
   54063900   235077000   0.000 26     2286000      381000 N   LEFT   DOWN 0   (Dexter)
Regular <Romansim Stroke Font>
PIVOT
    8250000    10200000  90.000 26     1905000      228600 N   LEFT   DOWN 0   (MAIS V5: no font line)
MAIS_FC001
```

## 11. `*MISC*`
`*MISC*` holds nested `KEYWORD VALUE` lines with `{` and `}` on their own lines. Empty containers have no braces at all: `NET_CLASS DATA` is followed directly by `GROUP DATA`. Track brace depth. Top-level blocks seen:
- `LAYER DATA`
- `RULES_SECTION PARENT`
- `ATTRIBUTES DICTIONARY`
- `ATTRIBUTE VALUES`
- `VISIBILITY DATA`, `SELECTABILITY DATA`, `CAM_SECTION PARENT`, `DFT_CONFIGURATION PARENT`, `DOCUMENTS_LIST PARENT`, `ATPP_CONFIGURATION PARENT`: ignore these.

### 11.1 LAYER DATA
Read it.
```
LAYER DATA
{
LAYER 0            <- stack base: LAYER_THICKNESS, DIELECTRIC only
{ ... }
LAYER 1
{
LAYER_NAME Top
LAYER_TYPE ROUTING          ROUTING | PLANE? | MIXED? | UNASSIGNED | SOLDER_MASK | PASTE_MASK | SILK_SCREEN | ASSEMBLY | DRILL
PLANE MIXED                 NONE | MIXED (| CAM)
ROUTING_DIRECTION NO_PREFERENCE
ASSOCIATED_SILK_SCREEN Silkscreen Top        <- top/bottom pairing of documentation layers
ASSOCIATED_PASTE_MASK Paste Mask Top
ASSOCIATED_SOLDER_MASK Solder Mask Top
ASSOCIATED_ASSEMBLY Assembly Drawing Top
COMPONENT Y ... COLORS : { ... }
LAYER_THICKNESS 381000      dielectric below this layer (length)
COPPER_THICKNESS 51435      copper foil (length)
DIELECTRIC 4.300000
NET DGND                    plane net(s), repeatable
COST 0
}
...
}
```
This is Dexter's layer 1, abridged.

- A layer is copper if 1 ≤ number ≤ MAXIMUMLAYER. The `LAYER_TYPE` of layers above N is `UNASSIGNED`.
- Classify non-copper layers by LAYER_TYPE, and use the name to decide top or bottom ("bottom"/"bot"). KiCad does exactly this.
- Layer names are free text and may contain spaces (`Inner Layer 1 (GND)`). Take the rest of the line after `LAYER_NAME `.

### 11.2 Design rules
These sit under `RULES_SECTION PARENT { NET_CLASS DATA … GROUP DATA … DESIGN RULES { RULE_SET (k) { FOR : {…} AGAINST : {…} LAYER 0 CLEARANCE_RULE : {…} } … } }`.

- **`RULE_SET (1)` with `FOR : { DEFAULT : }` is the default rule set.** Values are lengths. TMS example:
  - `TRACK_TO_TRACK 304800` (8 mil)
  - `VIA_TO_TRACK 381000`, `PAD_TO_TRACK 228600`, `SMD_TO_SMD 133350`, `COPPER_TO_COPPER 762000`
  - `OUTLINE_TO_TRACK 381000`, `DRILL_TO_DRILL 381000`
  - `MIN_TRACK_WIDTH 228600`, `REC_TRACK_WIDTH 381000` (default track width), `MAX_TRACK_WIDTH 3810000`
- Other rule sets use `FOR : { NET <name> }` (Dexter has 33 sets) or `NET_CLASS <name>`. Their inner blocks are `HIGH_SPEED_RULE :` and `ROUTE_RULE :`, which list `VALID_LAYER n` and `VALID_VIA_TYPE <via>`.
- KiCad derives its rules as follows: clearance = the minimum over the `TRACK_TO_TRACK`, `VIA_TO_*`, `PAD_TO_*`, `SMD_TO_*` and `DRILL_TO_*` rules; edge clearance = the minimum over the `OUTLINE_TO_*` rules; default track width = `REC_TRACK_WIDTH`.
- Via sizes come from `*VIA*`.
- Net classes appear as `NET_CLASS <name> { NET <n> … }` inside `NET_CLASS DATA` (empty in all samples).

### 11.3 Attributes
`ATTRIBUTE VALUES { PART <ref> { "<attr name>" <value> … } NET <name> { … } }`. Part values such as "Value" and per-net strategies live here. Attribute names may be quoted.

## 12. Pitfalls found in the samples
1. **Dialect drift.** Font lines, LINESTYLE, CORNER, T pin names, the PARTTYPE UNITS field and the LINES FLAGS field all depend on the version (§1.2). Check the file's `*REMARK*` lines, but do not require them: ClaySight has none.
2. **Numeric names.** Ems4 has refdes `1` and `2`, and connection lines such as `2.1 4.1`. TMS has decals named `0402` and `0805`. Classify lines by token shape and record counts, never by their first character.
3. **Duplicated route segments and vias** (§7). De-duplicate them.
4. **Two arc encodings:** an arc vertex in LINES and decals (§3.3) versus a centre line in POUR (§9). Route arcs (CW/CCW) are a third encoding and are absent from the samples.
5. **Different RT row order** in `*VIA*` versus decal stacks (§8).
6. **Non-copper layer numbers vary by file and by 250L mode.** Always use LAYER DATA.
7. **The decal unit letter is irrelevant in BASIC files.** TMS mixes `I` and `M` decals with identical scaling.
8. **Empty sections are legal.** ClaySight has empty `*POUR*`, `*CLUSTER*` and `*VIA*`, and the PADS-native files have an empty `*REUSE*` (REMARKs only) and a `*TESTPOINT*` with no content. Some samples have no `*POUR*` at all (TMS).
9. **Bad bytes.** Dexter has bytes ≥ 0x80 in `*MISC*`.
10. **Labels in decals and parts sit on copper layer numbers** (level 1) and on assembly layers. They are documentation, not copper.
11. **Trailing whitespace and variable padding everywhere,** plus trailing comments in `*PCB*` and `; (All)` in `*SIGNAL*`.
12. **EasyEDA (ClaySight) quirks:** layer changes with no via (§7), a first route point slightly off the pad joined by a layer-0 stub, `NONE`-visibility labels named `Name`, `Device`, `Footprint` …, and integer angles.
13. **Pad drill only on the −2 row.** Inner and bottom rows never repeat it.

## 13. Writer checklist and minimal example

**What to emit** (BASIC units, V9.5 dialect: the best-tested path through KiCad and identical to TMS and Ems4):
1. The header line `!PADS-POWERPCB-V9.5-BASIC! DESIGN DATABASE ASCII FILE 1.0`. KiCad needs `!PADS-` on line 1.
2. `*PCB*` with at least `UNITS 0`, `MAXIMUMLAYER N` and `ORIGIN 0 0`. Keep all coordinates positive (§1.4).
3. `*TEXT*` (may be empty).
4. `*LINES*` with exactly one `BOARD` item.
5. `*VIA*` defining every via name used in `*ROUTE*`.
6. `*PARTDECAL*`, before the part types and parts that reference it.
7. `*PARTTYPE*` with one entry per part type, DECALNM = decal (or a `:` list).
8. `*PART*`.
9. `*ROUTE*` with **one `*SIGNAL*` per net of ≥ 2 pins**. Write every net, routed or not; an unrouted connection is 2 points: layer 0 at pin A, then layer 65 at pin B.
10. `*POUR*` (optional).
11. `*MISC*` containing `LAYER DATA`, which KiCad needs to place silk and mask layers correctly.
12. `*END*`.

Use ASCII, LF, single spaces and integer BASIC values. Optionally prefix each section with the PADS `*REMARK*` lines for readability. Whether real PADS Layout accepts a file this minimal is [unverified]. ClaySight is a minimal-style EasyEDA export of the same kind.

**Example:** a 40×30 mm 2-layer board, one through-hole resistor R1 (pins 200 mil apart), net N1 from R1.1 to R1.2: top track → via → bottom track. 1 mm = 1 500 000.
```
!PADS-POWERPCB-V9.5-BASIC! DESIGN DATABASE ASCII FILE 1.0
*PCB*
UNITS        0                     display units: mils (cosmetic in BASIC)
MAXIMUMLAYER 2                     2 copper layers: 1=Top, 2=Bottom
ORIGIN       0 0
*TEXT*
*LINES*
OUTLINE      BOARD    0 0 1 0      name BOARD x y pieces=1 flags=0
CLOSED 5   152400 0   0            closed poly, 5 corners, 4 mil width, linestyle 0, level 0
0 0
60000000 0
60000000 45000000
0 45000000
0 0                                last corner repeats the first
*VIA*
STANDARDVIA      762000 3          drill 20 mil, 3 rows
-2 1524000 R                       top pad 40 mil
-1 1524000 R                       inner
0  1524000 R                       bottom
*PARTDECAL*
RES200 I 0 0 1 2 1 0 2             1 piece, 2 terminals, 1 stack, 0 text, 2 labels
OPEN 2   254000 0   26             silk line, 10 mil, layer 26 (Silkscreen Top per MISC below)
-2286000 0
2286000 0
VALUE 0 2286000 0.000 26 1905000 190500 N CENTER CENTER     label 1: refdes, 50 mil high
Regular <Romansim Stroke Font>
Ref.Des.
VALUE 0 -2286000 0.000 27 1905000 190500 N CENTER CENTER    label 2: part type on assembly top
Regular <Romansim Stroke Font>
Part Type
T-3810000 0 -3810000 0 1           terminal 1 at (-100 mil, 0), pin name "1"
T3810000 0 3810000 0 2             terminal 2 at (+100 mil, 0), pin name "2"
PAD 0 3                            default stack for all terminals
-2 2286000 R 1143000 P             60 mil round, 30 mil plated drill (drill on -2 row only)
-1 2286000 R
0  2286000 R
*PARTTYPE*
RES RES200 RES 1 0 0 0 Y           1 gate, 0 sigpins, 0 unused, flags 0, ECO Y
G 0 2                              gate: swap 0, 2 pins
1.0.U. 2.0.U.
*PART*
R1 RES 30000000 22500000 0.000 U N 0 -1 0 -1 0   at (20mm,15mm), 0 deg, unglued, top, alt 0, no labels
*ROUTE*
*SIGNAL* N1 0 -2
R1.1 R1.2                                         connection R1.1 -> R1.2
26190000 22500000 1 381000 1792 THERMAL           at pin 1; segment to next on layer 1, 10 mil
26190000 30000000 1 381000 1536                   corner; next segment still layer 1
30000000 30000000 2 381000 1792 STANDARDVIA THERMAL  via here; next segment on layer 2
33810000 30000000 2 381000 1536                   corner on layer 2
33810000 22500000 65 381000 1792 THERMAL          at pin 2; 65 = end of connection
*MISC*      MISCELLANEOUS PARAMETERS
LAYER DATA
{
LAYER 1
{
LAYER_NAME Top
LAYER_TYPE ROUTING
PLANE NONE
}
LAYER 2
{
LAYER_NAME Bottom
LAYER_TYPE ROUTING
PLANE NONE
}
LAYER 21
{
LAYER_NAME Solder Mask Top
LAYER_TYPE SOLDER_MASK
}
LAYER 26
{
LAYER_NAME Silkscreen Top
LAYER_TYPE SILK_SCREEN
}
LAYER 27
{
LAYER_NAME Assembly Drawing Top
LAYER_TYPE ASSEMBLY
}
LAYER 28
{
LAYER_NAME Solder Mask Bottom
LAYER_TYPE SOLDER_MASK
}
LAYER 29
{
LAYER_NAME Silkscreen Bottom
LAYER_TYPE SILK_SCREEN
}
}
*END*     OF ASCII OUTPUT FILE
```
Checks:
- Pin 1 = (30000000 − 3810000, 22500000) = (26190000, 22500000) and pin 2 = (33810000, 22500000), so the route's end points are exactly on the pins, as PADS writes them.
- The via point carries the outgoing layer (2), and the `STANDARDVIA` name matches `*VIA*`.
- The annotation text to the right of each line is for this document only. Strip it before writing. The `*PCB*` section is the only place where PADS itself tolerates trailing comments.
- The font line could be omitted for V5, but keep it for V9.5.


---
The six sample boards are from KiCad's PADS importer test data (GPL-3.0) and are not included in this repository; Coppersmith's tests read them from `~/Documents/examplelayouts/pads` (or `COPPERSMITH_PADS_DIR`) when they are there.
