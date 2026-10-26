# Launcher UI sprites (W98)

All new sprites are derived from imagegen output. Coordinates below are half-open source pixel boxes: `[left, top, right, bottom)`. The source sheet is normalized to flat `#FF00FF` with at least 48 px between placements; keyed edges are despilled and the delivered sprites are trimmed.

## Source images

| Source | SHA-256 | Notes |
|---|---|---|
| `sheet-a2-corrected.png` | `52dcf557d1cf4cc50a7da50d33f25be5890a0a9d15d252efb11a57fe9092069f` | edited generated component sheet; normalized copy is `lab/runs/launcher-art/w98/sheet-a2.png` |
| `icons.png` | `a205fc2ea3a31b1a4dd8454e827cc6a7fdced24d6b103c8aaaef0d95448d106d` | reused icon sheet from the approved first attempt |
| `vines.png` | `b72398963cc87c90c37c12e2e03ca98c23d8802b0e11d041b19afed0b58a9149` | generated orange vine tendrils |
| `backdrop.png` | `40ad977675b394dc40dc22382101f3fa36cc42996bf88ca9e951d2232c512a8d` | reused approved forest backdrop |

## Visual references (not cropped)

| Reference | SHA-256 | Use |
|---|---|---|
| `mockup-a.png` | `56d0323894054a9fe261844011a04e996f506a644e849c89e978190bbe8f2ce7` | approved composition and color reference |
| `sheet-a.png` | `57b85ab4b731daf1ec59bb873196e0ca87deed8c71753cbe924b6c8fe7a91c26` | original component-sheet layout reference |
| `project/docs/images/banner.png` | `161f6e944229405d8954057aa112afac51ec92dfb89fbcc72fdff2f88dbc4bdb` | wordmark placement reference; bundled banner unchanged |
| `ref-fr-ui-kit.png` | `c601a24edd69a7b18a9d5a168fc5fff23a1d8de12c5cb1500974f11d7c2dbed1` | UI-kit color and rim reference |

## Slice map

| Sprite | Source | Source box | Delivered size | Vine root |
|---|---|---:|---:|---:|
| `button_normal.png` | `sheet-a2-corrected.png` | `[44, 84, 381, 183)` | `337 x 99` | n/a |
| `button_hover.png` | `sheet-a2-corrected.png` | `[414, 84, 750, 183)` | `336 x 99` | n/a |
| `button_pressed.png` | `sheet-a2-corrected.png` | `[783, 84, 1119, 183)` | `336 x 99` | n/a |
| `button_disabled.png` | `sheet-a2-corrected.png` | `[1152, 84, 1492, 183)` | `340 x 99` | n/a |
| `play_normal.png` | `sheet-a2-corrected.png` | `[43, 244, 440, 367)` | `397 x 123` | n/a |
| `play_hover.png` | `sheet-a2-corrected.png` | `[460, 244, 858, 366)` | `398 x 122` | n/a |
| `tab_selected.png` | `sheet-a2-corrected.png` | `[885, 252, 1171, 361)` | `286 x 109` | n/a |
| `tab_unselected.png` | `sheet-a2-corrected.png` | `[1202, 251, 1489, 361)` | `287 x 110` | n/a |
| `panel.png` | `sheet-a2-corrected.png` | `[46, 408, 612, 808)` | `566 x 400` | n/a |
| `list_row.png` | `sheet-a2-corrected.png` | `[707, 497, 1304, 601)` | `597 x 104` | n/a |
| `text_field.png` | `sheet-a2-corrected.png` | `[707, 659, 1425, 739)` | `718 x 80` | n/a |
| `pip_online.png` | `sheet-a2-corrected.png` | `[65, 861, 150, 944)` | `40 x 39` | n/a |
| `pip_pending.png` | `sheet-a2-corrected.png` | `[219, 861, 304, 944)` | `40 x 39` | n/a |
| `pip_error.png` | `sheet-a2-corrected.png` | `[383, 861, 467, 944)` | `40 x 40` | n/a |
| `pip_disabled.png` | `sheet-a2-corrected.png` | `[544, 861, 630, 944)` | `40 x 39` | n/a |
| `close_x.png` | `sheet-a2-corrected.png` | `[744, 852, 841, 949)` | `64 x 64` | n/a |
| `plank.png` | `sheet-a2-corrected.png` | `[922, 844, 1484, 955)` | `562 x 111` | n/a |
| `icon_leaf_green.png` | `icons.png` | `[232, 189, 570, 540)` | `128 x 133` | n/a |
| `icon_leaf_blue.png` | `icons.png` | `[901, 189, 1210, 540)` | `128 x 145` | n/a |
| `icon_sprig_blue.png` | `icons.png` | `[1542, 148, 1935, 503)` | `128 x 116` | n/a |
| `vine_1.png` | `vines.png` | `[58, 108, 753, 410)` | `400 x 174` | `(10, 161)` |
| `vine_2.png` | `vines.png` | `[840, 83, 1476, 453)` | `400 x 200` | `(11, 180)` |
| `vine_3.png` | `vines.png` | `[56, 557, 746, 853)` | `400 x 172` | `(8, 161)` |
| `vine_4.png` | `vines.png` | `[795, 519, 1478, 919)` | `400 x 200` | `(9, 189)` |
| `backdrop.png` | `backdrop.png` | `[0, 0, 1672, 941)` | `1280 x 720` | n/a |

The vine roots are local delivered-image coordinates, measured at the cut end that anchors on the banner wordmark. Four to 39 detached antialias specks per vine were removed; every remaining opaque pixel belongs to the root-connected component used by the runtime BFS reveal.

`slices.json` is the complete machine-readable record, including sampled magenta key values, normalized-atlas placements and detached-pixel counts.

## W112 polish (2026-10-16)

W112 keeps the W98 panel, buttons, tabs and vine source art. It reslices only the
realm row, text field and hint plank from the existing generated `sheet-a.png`
(SHA-256 `57b85ab4b731daf1ec59bb873196e0ca87deed8c71753cbe924b6c8fe7a91c26`).
No promotional art was cut out or added. The normalized banner protection boxes
and all slice metadata are in `slices.json`.

| Sprite | Source box | Trimmed box | Delivered size | Centre box | Rim |
|---|---:|---:|---:|---:|---|
| `list_row.png` | `[707, 497, 1304, 601)` | `[708, 498, 1303, 601)` | `595 x 103` | `[48, 17, 547, 86)` | smooth bevel |
| `text_field.png` | `[707, 659, 1425, 739)` | `[708, 663, 1423, 737)` | `715 x 74` | `[24, 16, 691, 58)` | smooth bevel |
| `plank.png` | `[922, 844, 1484, 955)` | `[928, 850, 1477, 954)` | `549 x 104` | `[60, 32, 489, 72)` | smooth bevel |

The panel uses a 400 px tile with 16 complete 25 px cyan bead periods. Measured
top-edge pitches differ across control sprites: normal, hover, pressed and
disabled buttons use 22, 25, 24 and 23 px; PLAY uses 24 px; tabs use 23 px.
The renderer preserves corner sizes, crops a centered strip with whole bead
periods and fits a whole tile count to the destination edge. The panel centre
box is plain; the quiet-control centres are tested against cyan and gold rim
pixel thresholds. Vines use roots `[10,161]`, `[11,180]`, `[8,161]`,
`[9,189]` and are anchored along the banner wordmark's right edge; a pixel test
checks full reveal and the 0.3/0.8 growth states against the protected wordmark,
starburst and other banner copy boxes.
