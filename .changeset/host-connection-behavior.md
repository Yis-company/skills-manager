---
"skills-manager": minor
---

A failed switch to another Host now keeps you on the Host you came from, with the
error and Retry. A dropped connection, or a Host that stops answering for 30 seconds,
reconnects quietly for about 30 seconds before it shows Reconnect. The Host switcher
stays usable while connecting, so you can cancel or pick another Host. Editing the
active Host reconnects it with the new details, and switching while skill installs
are unfinished asks whether to wait for them or cancel them.
