# Drawing together over Leyline

qsketch can act as a full-featured shared sketchpad for a
[Leyline](https://github.com/mayathegreat/leyline) conversation. Leyline is
the transport: it must be running (and connected) on the same computer, on
version 0.7.41 or newer.

## Using it

- **File ▸ Share via Leyline…** lists your contacts and groups. Pick one and
  press **Share "…"** to put the current canvas in that conversation, or
  **Join shared canvas** to open the conversation's canvas as a new tab.
- A shared tab wears the conversation's colored dot. The status bar shows the
  conversation and who else is drawing; their pointers appear on the canvas
  with their name and tool.
- **File ▸ Stop Sharing** leaves the conversation. The document stays open as
  a plain one, and can be saved like any other.
- Everything you can do to a normal document works on a shared one: brushes,
  filters, selections, transforms, layers and groups.

## What travels

qsketch sends **committed state**, not brush strokes: after every history
step it compares the document with the last state it sent and ships the tiles
that changed (as PNG patches), plus the layer stack when it changed, plus a
full snapshot now and then so a late joiner starts from a recent base. Your
pointer goes out as an ephemeral position.

What comes back from the others is written into the working state **and
every undo snapshot**, the same way layer visibility is, so:

- your undo only ever takes back *your own* marks;
- a peer's stroke is never re-sent as if it were yours;
- where two people paint the same tile at once, the later change (by
  Leyline's ordering counter) wins on both sides, so everyone converges.

Each participant gets its own range of layer ids, so two people adding a
layer at the same moment do not collide.

## How Leyline carries it

qsketch connects to Leyline's loopback *sketch link* (`127.0.0.1:8952`,
newline-delimited JSON; `QSKETCH_LEYLINE_PORT` overrides the port, and
`LEYLINE_SKETCH_LINK_PORT` on Leyline's side). Each change is an op in the
conversation's sketchpad log, so it gets Leyline's group fan-out, self-carbons
to your other devices, the offline queue and encrypted-at-rest storage for
free. Leyline's own sketchpad ignores these ops (they are kinds it does not
draw), and Leyline never needs to understand a layer.

Joining replays the stored log from its newest snapshot. If the log holds no
snapshot (it was cleared or compacted), the joiner asks the room and whoever
has the document open answers with a fresh one.
