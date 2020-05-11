#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import sys
from threading import Thread
from typing import Callable, List, Optional

from gi.repository import GLib

import dbus
import dbus.mainloop.glib

class LrcLineSegmentInfo:
    def __init__(self, dbus_data):
        self.line_index = int(dbus_data[0])
        self.line_char_from_index = int(dbus_data[1])
        self.line_char_to_index = int(dbus_data[2])

    def __str__(self):
        return "{}:{}-{}".format(
            self.line_index, self.line_char_from_index, self.line_char_to_index)

class LrcInfo:
    def __init__(self, logger: Callable):
        self.logger: Callable = logger

        self.lines: Optional[List[str]] = None
        self.active_segment: Optional[LrcLineSegmentInfo] = None

class SingleLineFormatter:
    def __init__(self, lrc_info: LrcInfo, logger: Callable):
        self.logger: Callable = logger
        self.lrc_info = lrc_info
        self.single_line = b""
        self._current_lyrics_text = None
        self.line_index_to_single_line_index_mapping: List[int] = []

    def _update_data_if_needed(self):
        if self.lrc_info.lines is None or self._current_lyrics_text is not self.lrc_info.lines:
            self._current_lyrics_text = self.lrc_info.lines
            self.line_index_to_single_line_index_mapping.clear()
            self.single_line = b""

            if self._current_lyrics_text is not None:
                for line in self._current_lyrics_text:
                    self.line_index_to_single_line_index_mapping.append(len(self.single_line))
                    self.single_line += line.encode(encoding='utf-8') + b"|"
                # For last line
                self.line_index_to_single_line_index_mapping.append(len(self.single_line))

    def get_as_single_line(self):
        self._update_data_if_needed()

        text_before = ""
        pre_active = ""
        active = ""
        post_active = ""
        text_after = ""
        if len(self.single_line) == 0 or self.lrc_info.active_segment is None:
            return (text_before, pre_active, active, post_active, text_after)

        active_segment = self.lrc_info.active_segment

        chars_before = 20
        max_width = 110
        active_line_start_index = self.line_index_to_single_line_index_mapping[active_segment.line_index]
        active_start_index = active_line_start_index + active_segment.line_char_from_index
        active_end_index = active_line_start_index + active_segment.line_char_to_index
        active_line_end_index = self.line_index_to_single_line_index_mapping[active_segment.line_index + 1]  # There should always be +1

        text_before_start_index = max(chars_before, active_start_index - chars_before)
        text_before = self.single_line[text_before_start_index:active_start_index].decode(encoding='utf-8')
        if active_start_index - chars_before < 0:
            text_before = " " * abs(active_start_index - chars_before) + text_before
        active = self.single_line[active_start_index:active_end_index].decode(encoding='utf-8')
        post_active = self.single_line[active_end_index:active_line_end_index].decode(encoding='utf-8')
        len_sum1 = len(text_before) + len(pre_active) + len(pre_active)  + len(active) + len(post_active)
        text_after_len = max_width - len_sum1
        assert text_after_len > 0
        text_after = self.single_line[active_line_end_index:active_line_end_index + text_after_len].decode(encoding='utf-8')

        total_len = len_sum1 + len(text_after)
        if total_len < max_width:
            text_after = text_after + (" " * (max_width - total_len))

        ret = (text_before, pre_active, active, post_active, text_after)
        self.logger("{}".format(ret))
        return ret

class LrcReceiver:
    def __init__(self, update_callback: Callable[[None], None], lrc_info: LrcInfo, logger: Callable[[str], None]):
        self.update_callback = update_callback
        self.logger = logger
        self.lrc_info: LrcInfo = lrc_info
        self.bus = None

    def _get_hal_manager(self):
        hal_manager_object = self.bus.get_object(
            'com.github.nikola_kocic.lrcshow_rs',
            '/com/github/nikola_kocic/lrcshow_rs/Lyrics')
        # Can't easily cache object because it should handle player restart
        return dbus.Interface(
            hal_manager_object, 'com.github.nikola_kocic.lrcshow_rs.Lyrics')

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
            data = self._get_hal_manager().GetCurrentLyricsPosition()
            self._on_active_lyrics_line_changed(data)
        except dbus.exceptions.DBusException as e:
            self.logger("Exception getting lyrics position: " + str(e))

    def _on_active_lyrics_line_changed(self, data):
        lyrics_position = LrcLineSegmentInfo(data)
        self.logger("Active lyrics line changed: {}".format(lyrics_position))
        if self.lrc_info.lines is None:
            self.lrc_info.lines = self._read_lyrics()
        if lyrics_position.line_index < 0:
            self.lrc_info.active_segment = None
        else:
            self.lrc_info.active_segment = lyrics_position
        self.update_callback()

    def _on_active_lyrics_changed(self):
        self.lrc_info.active_segment = None
        self.lrc_info.lines = self._read_lyrics()
        self._update_lyrics_position()
        self.update_callback()

    def start_loop(self):
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

        self.bus = dbus.SessionBus()

        self._on_active_lyrics_changed()

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


class Py3status:
    def __init__(self):
        self.update_thread = None
        self.lyrics_receiver = None
        self.lrc_info = None
        self.lrc_formatter = None

    def post_config_hook(self):
        log = lambda t: None
        # log = self.py3.log

        self.lrc_info = LrcInfo(log)
        self.lrc_formatter = SingleLineFormatter(self.lrc_info, log)
        self.lyrics_receiver = LrcReceiver(self.py3.update, self.lrc_info, log)

    def _start_handler_thread(self):
        """Called once to start the event handler thread."""
        self.update_thread = Thread(target=self.lyrics_receiver.start_loop)
        self.update_thread.daemon = True
        self.update_thread.start()

    def _get_composite_content(self):
        text_before, pre_active, active, post_active, text_after = self.lrc_formatter.get_as_single_line()

        return [
            {'full_text': text_before, 'color': '#808080'},
            {'full_text': pre_active},
            {'full_text': active, 'color': '#ff0000'},
            {'full_text': post_active},
            {'full_text': text_after, 'color': '#808080'},
        ]

    def lrcshowrs(self):
        if self.update_thread is None:
            self._start_handler_thread()

        return {
            'composite': self._get_composite_content(),
            'cached_until': self.py3.CACHE_FOREVER
        }


class bcolors:
    HEADER = '\033[95m'
    OKBLUE = '\033[94m'
    OKGREEN = '\033[92m'
    WARNING = '\033[93m'
    FAIL = '\033[91m'
    ENDC = '\033[0m'
    BOLD = '\033[1m'
    UNDERLINE = '\033[4m'


def print_to_stderr(t):
    sys.stderr.write(t)
    sys.stderr.write('\n')
    sys.stderr.flush()


class TerminalPrinter:
    def __init__(self):
        self.last_line_index = -1
        # log = lambda t: None
        log = print_to_stderr
        self.lrc_info = LrcInfo(log)
        self.lyrics_receiver = LrcReceiver(self.update, self.lrc_info, log)
        self.lyrics_receiver.start_loop()

    def update(self):
        lyrics_text = self.lrc_info.lines
        active_segment = self.lrc_info.active_segment

        if lyrics_text is None or active_segment is None or active_segment.line_index < 0:
            self.last_line_index = -1
            sys.stdout.write("\r{}".format(" " * 80))
            sys.stdout.write("\r")
        else:
            if active_segment.line_index != self.last_line_index:
                sys.stdout.write("\r{}".format(" " * 80))
                sys.stdout.write("\r{}\n".format(lyrics_text[active_segment.line_index]))
                self.last_line_index = active_segment.line_index
            sys.stdout.write("\r{}{}".format(
                '-' * active_segment.line_char_from_index,
                '^' * (active_segment.line_char_to_index - active_segment.line_char_from_index)
            ))
        sys.stdout.flush()


class SingleLineTerminalPrinter:
    def __init__(self):
        # log = lambda t: None
        log = print_to_stderr

        self.lrc_info = LrcInfo(log)
        self.lrc_formatter = SingleLineFormatter(self.lrc_info, log)
        self.lyrics_receiver = LrcReceiver(self.update, self.lrc_info, log)
        self.lyrics_receiver.start_loop()

    def update(self):
        text_len = 0
        new_line = ""
        if self.lrc_info.active_segment is not None:
            text_before, pre_active, active, post_active, text_after = self.lrc_formatter.get_as_single_line()
            text_len = len(text_before) + len(pre_active) + len(active) + len(post_active) + len(text_after)
            new_line = (
                text_before + bcolors.OKBLUE + pre_active +
                bcolors.BOLD + active + bcolors.ENDC +
                bcolors.OKBLUE + post_active + bcolors.ENDC + text_after
            )
        sys.stdout.write("\r{}{}".format(new_line, " " * (120 - text_len)))
        sys.stdout.flush()


if __name__ == '__main__':
    SingleLineTerminalPrinter()
