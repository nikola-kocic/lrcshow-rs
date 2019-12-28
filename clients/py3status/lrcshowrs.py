#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import sys
from threading import Thread

from gi.repository import GLib

import dbus
import dbus.mainloop.glib

class LrcInfo:
    def __init__(self, logger):
        self.logger = logger

        self.lyrics_text = None
        self.line_index = None
        self.line_char_from_index = None
        self.line_char_to_index = None

class SingleLineFormatter:
    def __init__(self, lrc_info, logger):
        self.logger = logger
        self.lrc_info = lrc_info
        self.single_line = ""
        self._current_lyrics_text = None
        self.line_index_to_single_line_index_mapping = []

    def _update_data_if_needed(self):
        if self.lrc_info.lyrics_text is None or self._current_lyrics_text is not self.lrc_info.lyrics_text:
            self._current_lyrics_text = self.lrc_info.lyrics_text
            self.line_index_to_single_line_index_mapping.clear()
            self.single_line = ""

            if self._current_lyrics_text is not None:
                for line in self._current_lyrics_text:
                    self.line_index_to_single_line_index_mapping.append(len(self.single_line))
                    self.single_line += line + "|"
                # For last line
                self.line_index_to_single_line_index_mapping.append(len(self.single_line))

    def get_as_single_line(self):
        self._update_data_if_needed()

        text_before = ""
        pre_active = ""
        active = ""
        post_active = ""
        text_after = ""
        if len(self.single_line) == 0 or self.lrc_info.line_index is None:
            return (text_before, pre_active, active, post_active, text_after)

        active_line_start_index = self.line_index_to_single_line_index_mapping[self.lrc_info.line_index]
        active_line_end_index = self.line_index_to_single_line_index_mapping[self.lrc_info.line_index + 1]  # There should always be +1

        active_line_len = active_line_end_index - active_line_start_index
        remaining_len_after = len(self.single_line) - active_line_end_index
        max_width = 110
        remaining_total_len = max_width - active_line_len
        desired_min_width_after = min(remaining_total_len, max(20, int((remaining_total_len * 3) / 4)))

        if remaining_len_after < remaining_total_len:  # We are near end, show as much text before active as we can
            text_after = self.single_line[active_line_end_index:]
            text_before_start_index = max(0, active_line_start_index - remaining_total_len + len(text_after))
            text_before = self.single_line[text_before_start_index:active_line_start_index]
        else:
            desired_width_before = remaining_total_len - desired_min_width_after
            text_before_start_index = max(0, active_line_start_index - desired_width_before)
            text_before = self.single_line[text_before_start_index:active_line_start_index]
            text_after_len = remaining_total_len - len(text_before)
            assert text_after_len > 0
            text_after = self.single_line[active_line_end_index:active_line_end_index + text_after_len]

        if self.lrc_info.line_char_to_index is not None and self.lrc_info.line_char_from_index is not None:
            active_start_index = active_line_start_index + self.lrc_info.line_char_from_index
            post_active_start_index = active_line_start_index + self.lrc_info.line_char_to_index

            pre_active = self.single_line[active_line_start_index:active_start_index]
            active = self.single_line[active_start_index:post_active_start_index]
            post_active = self.single_line[post_active_start_index:active_line_end_index]
        else:
            active = self.single_line[active_line_start_index:active_line_end_index]

        ret = (text_before, pre_active, active, post_active, text_after)
        self.logger("{}".format(ret))
        return ret

class LrcReceiver:
    def __init__(self, update_callback, lrc_info, logger):
        self.update_callback = update_callback
        self.logger = logger
        self.lrc_info = lrc_info
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
            lyrics_position = self._get_hal_manager().GetCurrentLyricsPosition()
            if lyrics_position[0] < 0:
                self.lrc_info.line_index = None
                self.lrc_info.line_char_from_index = None
                self.lrc_info.line_char_to_index = None
            else:
                self.lrc_info.line_index = int(lyrics_position[0])
                self.lrc_info.line_char_from_index = int(lyrics_position[1])
                self.lrc_info.line_char_to_index = int(lyrics_position[2])
            self.logger("Got lyrics position: " + str(lyrics_position))
        except dbus.exceptions.DBusException as e:
            self.logger("Exception getting lyrics position: " + str(e))

    def _on_active_lyrics_line_changed(
            self, line_index, line_char_from_index, line_char_to_index, *args, **kwargs):
        self.logger("_on_active_lyrics_line_changed: {}: {}-{}".format(line_index, line_char_from_index, line_char_to_index))
        if self.lrc_info.lyrics_text is None:
            self.lrc_info.lyrics_text = self._read_lyrics()
        if line_index < 0:
            self.lrc_info.line_index = None
            self.lrc_info.line_char_from_index = None
            self.lrc_info.line_char_to_index = None
        else:
            self.lrc_info.line_index = int(line_index)
            self.lrc_info.line_char_from_index = int(line_char_from_index)
            self.lrc_info.line_char_to_index = int(line_char_to_index)
        self.update_callback()

    def _on_active_lyrics_changed(self):
        self.lrc_info.line_index = None
        self.lrc_info.line_char_from_index = None
        self.lrc_info.line_char_to_index = None
        self.lrc_info.lyrics_text = self._read_lyrics()
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
        lyrics_text = self.lrc_info.lyrics_text
        line_index = self.lrc_info.line_index
        line_char_from_index = self.lrc_info.line_char_from_index
        line_char_to_index = self.lrc_info.line_char_to_index

        if lyrics_text is None or line_index is None or line_index < 0:
            self.last_line_index = -1
            sys.stdout.write("\r{}".format(" " * 80))
            sys.stdout.write("\r")
        else:
            if line_index != self.last_line_index:
                sys.stdout.write("\r{}".format(" " * 80))
                sys.stdout.write("\r{}\n".format(lyrics_text[line_index]))
                self.last_line_index = line_index
            sys.stdout.write("\r{}{}".format(
                '-' * line_char_from_index,
                '^' * (line_char_to_index - line_char_from_index)
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
        if self.lrc_info.line_index is not None:
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
