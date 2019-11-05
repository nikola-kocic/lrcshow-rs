#!/usr/bin/env python3

import gobject

import dbus
import dbus.mainloop.glib

def on_active_lyrics_line_changed(line_index, line_char_from_index, line_char_to_index):
    print(line_index, line_char_from_index, line_char_to_index)

if __name__ == '__main__':
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

    bus = dbus.SessionBus()

    bus.add_signal_receiver(
        on_active_lyrics_line_changed,
        dbus_interface = "com.github.nikola_kocic.lrcshow_rs.Daemon",
        signal_name = "ActiveLyricsSegmentChanged")

    loop = gobject.MainLoop()
    loop.run()
