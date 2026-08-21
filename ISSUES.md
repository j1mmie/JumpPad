# Known issues:

Occurs consistently when resizing the window super small on Windows
```
thread 'main' (21148) panicked at /rustc/8bab26f4f68e0e26f0bb7960be334d5b520ea452/library\core\src\num\f32.rs:1565:9:
min > max, or either was NaN. min = 28.0, max = 26.8
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
error: process didn't exit successfully: `target\release\jumppad-gpu.exe` (exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)
```

--

New tab button background color is same as active tab background color.
New tab button background should be same as tab bar background color.

--

With example text:
```
ABC

123
```

Placing caret at end of line ABC, press shift down. No text is visibly selected/highlighted AND the caret is gone.

Notepad and other text editors solve this by highlighting an invisible character after a line (after the ABC line, for example), to represent that a "newline" is highlighted.

--

Cursor should reset blink whenever window gains focus. Whenever the window gains focus, the cursor should immediately be visible.

--

Need support for custom word separator charactors. Similar to VSCode's `editor.wordSeparators`

--

Sometimes, when dragging the scrollbar thumb, it doesn't quite go to the limit that it should. It sort of stops before it should, and stops revealing all the lines in the file. For example, this file. If you scroll to the bottom of the file, then drag the thumb up, the thumb stops somewhere before the very top. It shows a line almost near the top of the file, but you can actually scroll up a bit further (using mousewheel, up arrows, etc). Same thing when scrolling down. I think what's happening is the line count is being miscalculated. It's not taking into account the fact that long lines wrap and actually occupy multiple lines.
