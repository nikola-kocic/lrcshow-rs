#!/usr/bin/env python3

import sys

from gi.repository import GLib

import dbus
import dbus.mainloop.glib

lyrics_text = None
last_line_index = -1

def read_lyrics():
    global lyrics_text
    hal_manager_object = bus.get_object('com.github.nikola_kocic.lrcshow_rs', '/com/github/nikola_kocic/lrcshow_rs/Lyrics')
    hal_manager_interface = dbus.Interface(hal_manager_object, 'com.github.nikola_kocic.lrcshow_rs.Lyrics')
    lyrics_text_raw = hal_manager_interface.GetCurrentLyrics()
    lyrics_text = [str(x) for x in lyrics_text_raw]
    print(lyrics_text)

def on_active_lyrics_line_changed(line_index, line_char_from_index, line_char_to_index):
    global last_line_index
    if lyrics_text is None:
        read_lyrics()
    # print(line_index, line_char_from_index, line_char_to_index)
    if line_index > last_line_index:
        sys.stdout.write("\r{}".format(" " * 80))
        sys.stdout.write("\r{}".format(lyrics_text[line_index]))
        last_line_index = line_index
        print()
    sys.stdout.write("\r{}{}".format('-' * line_char_from_index, '^' * (line_char_to_index - line_char_from_index)))
    sys.stdout.flush()

if __name__ == '__main__':
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

    bus = dbus.SessionBus()
    print()
    if lyrics_text is None:
        read_lyrics()

    bus.add_signal_receiver(
        on_active_lyrics_line_changed,
        dbus_interface = "com.github.nikola_kocic.lrcshow_rs.Daemon",
        signal_name = "ActiveLyricsSegmentChanged")

    loop = GLib.MainLoop()
    loop.run()
