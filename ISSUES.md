# Known issues:

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
