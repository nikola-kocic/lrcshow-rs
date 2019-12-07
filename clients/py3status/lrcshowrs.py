# -*- coding: utf-8 -*-

import sys
from threading import Thread

from gi.repository import GLib

import dbus
import dbus.mainloop.glib

class Py3status:
    def _read_lyrics(self):
        hal_manager_object = self.bus.get_object('com.github.nikola_kocic.lrcshow_rs', '/com/github/nikola_kocic/lrcshow_rs/Lyrics')
        hal_manager_interface = dbus.Interface(hal_manager_object, 'com.github.nikola_kocic.lrcshow_rs.Lyrics')
        lyrics_text_raw = hal_manager_interface.GetCurrentLyrics()
        self.lyrics_text = [str(x) for x in lyrics_text_raw]
        self.py3.log("New lyrics: " + str(self.lyrics_text))

    def _on_active_lyrics_line_changed(self, line_index, line_char_from_index, line_char_to_index):
        self.py3.log("_on_active_lyrics_line_changed: " + str(line_index))
        if self.lyrics_text is None:
            self._read_lyrics()
        self.line_index = line_index
        self.line_char_from_index = line_char_from_index
        self.line_char_to_index = line_char_to_index
        self.py3.update()

    def _on_active_lyrics_changed(self):
        self._read_lyrics()
        self._reset_locations()
        self.py3.update()

    def _start_loop(self):
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

        self.bus = dbus.SessionBus()
        if self.lyrics_text is None:
            self._read_lyrics()

        self.bus.add_signal_receiver(
            self._on_active_lyrics_line_changed,
            dbus_interface = "com.github.nikola_kocic.lrcshow_rs.Daemon",
            signal_name = "ActiveLyricsSegmentChanged")

        self.bus.add_signal_receiver(
            self._on_active_lyrics_changed,
            dbus_interface = "com.github.nikola_kocic.lrcshow_rs.Daemon",
            signal_name = "ActiveLyricsChanged")

        loop = GLib.MainLoop()
        loop.run()

    def _reset_locations(self):
        self.line_index = None
        self.line_char_from_index = None
        self.line_char_to_index = None

    def _start_handler_thread(self):
        """Called once to start the event handler thread."""
        self.update_thread = Thread(target=self._start_loop)
        self.update_thread.daemon = True
        self.update_thread.start()

    def post_config_hook(self):
        self.update_thread = None
        self.bus = None
        self.lyrics_text = None
        self._reset_locations()

    def _get_composite_content(self):
        if self.lyrics_text is None or self.line_index is None or (self.line_index > len(self.lyrics_text) - 1):
            return [{'full_text': ''}]

        active_line = self.lyrics_text[self.line_index]
        previous_line = (self.lyrics_text[self.line_index - 1] + " | ") if self.line_index > 0 else ""
        next_line = (" | " + self.lyrics_text[self.line_index + 1]) if (self.line_index + 1 < len(self.lyrics_text)) else ""

        if self.line_char_to_index is not None and self.line_char_from_index is not None:
            pre_active = active_line[0:self.line_char_from_index]
            active = active_line[self.line_char_from_index:self.line_char_to_index]
            post_active = active_line[self.line_char_to_index:]
        else:
            pre_active = ""
            active = active_line
            post_active = ""

        return [
            {'full_text': previous_line, 'color': '#808080'},
            {'full_text': pre_active},
            {'full_text': active, 'color': '#ff0000'},
            {'full_text': post_active},
            {'full_text': next_line, 'color': '#808080'},
        ]

    def lrcshowrs(self):
        if self.update_thread is None:
            self._start_handler_thread()

        return {
            'composite': self._get_composite_content(),
            'cached_until': self.py3.CACHE_FOREVER
        }
