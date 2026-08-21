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
