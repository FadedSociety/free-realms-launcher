# GM tools: credits and sources

- `gm_panel.as` is the Referee Tools panel from Sanctuary PR #163 by BubbaJoeX
  (https://github.com/Open-Source-Free-Realms/Sanctuary/pull/163, commit e9242e605af5b6cd5cbcd97ce5e4f40d26751ee0),
  AGPL-3.0, unchanged.
- `gm_hub_button.as` is adapted from the same PR's `gm_hub_button.as`: tinted stock tree icons instead of
  recoloured copies built into the movie.
- `*.doaction` are those scripts compiled (JPEXS FFDec 26.3.0, AS2) into SWF DoAction tag records. They hold only
  this code: the launcher adds them after the stock frame script of the player's own `chatWindowCtrl.gfx` (frame 2)
  and `gamedock.gfx` (frame 1), so no game files are in this repository.

Rebuild a `.doaction`: replace the frame's DoAction of a scratch copy of the stock movie with the script
(`ffdec -replace in.gfx out.gfx "\frame_2\DoAction" gm_panel.as`) and copy that frame's DoAction tag records out of
`out.gfx`.
