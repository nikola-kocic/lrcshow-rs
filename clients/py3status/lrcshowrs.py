# -*- coding: utf-8 -*-

import sys
from threading import Thread

from gi.repository import GLib

import dbus
import dbus.mainloop.glib


class LrcReceiver:
    def __init__(self, update_callback, logger):
        self.update_callback = update_callback
        self.logger = logger

        self.bus = None
        self.lyrics_text = None
        self.line_index = None
        self.line_char_from_index = None
        self.line_char_to_index = None
        self.hal_manager_interface = None

    def _get_hal_manager(self):
        if self.hal_manager_interface is None:
            hal_manager_object = self.bus.get_object(
                'com.github.nikola_kocic.lrcshow_rs',
                '/com/github/nikola_kocic/lrcshow_rs/Lyrics')
            self.hal_manager_interface = dbus.Interface(
                hal_manager_object, 'com.github.nikola_kocic.lrcshow_rs.Lyrics')
        return self.hal_manager_interface

    def _read_lyrics(self):
        lyrics_text = None

        try:
            lyrics_text_raw = self._get_hal_manager().GetCurrentLyrics()
            lyrics_text = [str(x) for x in lyrics_text_raw]
            self.logger("New lyrics: " + str(lyrics_text))
        except dbus.exceptions.DBusException as e:
            self.logger("Exception getting lyrics: " + str(e))

        return lyrics_text

    def _update_lyrics_position(self):
        try:
            lyrics_position = self._get_hal_manager().GetCurrentLyricsPosition()
            if lyrics_position[0] < 0:
                self.line_index = None
                self.line_char_from_index = None
                self.line_char_to_index = None
            else:
                self.line_index = lyrics_position[0]
                self.line_char_from_index = lyrics_position[1]
                self.line_char_to_index = lyrics_position[2]
            self.logger("Got lyrics position: " + str(lyrics_position))
        except dbus.exceptions.DBusException as e:
            self.logger("Exception getting lyrics position: " + str(e))

    def _on_active_lyrics_line_changed(
            self, line_index, line_char_from_index, line_char_to_index, *args, **kwargs):
        self.logger("_on_active_lyrics_line_changed: " + str(line_index))
        if self.lyrics_text is None:
            self.lyrics_text = self._read_lyrics()
        if line_index < 0:
            self.line_index = None
            self.line_char_from_index = None
            self.line_char_to_index = None
        else:
            self.line_index = line_index
            self.line_char_from_index = line_char_from_index
            self.line_char_to_index = line_char_to_index
        self.update_callback()

    def _on_active_lyrics_changed(self):
        self.line_index = None
        self.line_char_from_index = None
        self.line_char_to_index = None
        self.lyrics_text = self._read_lyrics()
        self.update_callback()

    def start_loop(self):
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

        self.bus = dbus.SessionBus()

        self.lyrics_text = self._read_lyrics()
        self._update_lyrics_position()
        self.update_callback()

        self.bus.add_signal_receiver(
            self._on_active_lyrics_line_changed,
            dbus_interface="com.github.nikola_kocic.lrcshow_rs.Daemon",
            signal_name="ActiveLyricsSegmentChanged")

        self.bus.add_signal_receiver(
            self._on_active_lyrics_changed,
            dbus_interface="com.github.nikola_kocic.lrcshow_rs.Daemon",
            signal_name="ActiveLyricsChanged")

        loop = GLib.MainLoop()
        loop.run()

    def has_valid_lyrics(self):
        return (
            self.lyrics_text is not None
            and self.line_index is not None
            and self.line_index < len(self.lyrics_text)
        )


class Py3status:
    def __init__(self):
        self.update_thread = None
        self.lyrics_receiver = None

    def post_config_hook(self):
        log = lambda t: None
        # log = self.py3.log
        self.lyrics_receiver = LrcReceiver(self.py3.update, log)

    def _start_handler_thread(self):
        """Called once to start the event handler thread."""
        self.update_thread = Thread(target=self.lyrics_receiver.start_loop)
        self.update_thread.daemon = True
        self.update_thread.start()

    def _get_composite_content(self):
        if not self.lyrics_receiver.has_valid_lyrics():
            return [{'full_text': ''}]

        lyrics_text = self.lyrics_receiver.lyrics_text
        line_index = self.lyrics_receiver.line_index
        line_char_from_index = self.lyrics_receiver.line_char_from_index
        line_char_to_index = self.lyrics_receiver.line_char_to_index

        active_line = lyrics_text[line_index]
        previous_line = (lyrics_text[line_index - 1] + " | "
                         if line_index > 0 else "")
        next_line = (" | " + lyrics_text[line_index + 1]
                     if (line_index + 1 < len(lyrics_text)) else "")

        if line_char_to_index is not None and line_char_from_index is not None:
            pre_active = active_line[0:line_char_from_index]
            active = active_line[line_char_from_index:line_char_to_index]
            post_active = active_line[line_char_to_index:]
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


class TerminalPrinter:
    def __init__(self):
        self.last_line_index = -1

        log = lambda t: None
        # log = print
        self.lyrics_receiver = LrcReceiver(self.update, log)
        self.lyrics_receiver.start_loop()

    def update(self):
        if not self.lyrics_receiver.has_valid_lyrics():
            self.last_line_index = -1
            sys.stdout.write("\r{}".format(" " * 80))
            sys.stdout.write("\r")
        else:
            lyrics_text = self.lyrics_receiver.lyrics_text
            line_index = self.lyrics_receiver.line_index
            line_char_from_index = self.lyrics_receiver.line_char_from_index
            line_char_to_index = self.lyrics_receiver.line_char_to_index

            if line_index != self.last_line_index:
                sys.stdout.write("\r{}".format(" " * 80))
                sys.stdout.write("\r{}\n".format(lyrics_text[line_index]))
                self.last_line_index = line_index
            sys.stdout.write("\r{}{}".format(
                '-' * line_char_from_index,
                '^' * (line_char_to_index - line_char_from_index)
            ))
        sys.stdout.flush()


if __name__ == '__main__':
    TerminalPrinter()
