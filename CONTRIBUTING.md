# Contributing to JumpPad

Please follow the guidelines below when possible. Of course, all rules may
have exceptions, but they should be considered carefully.

### Naming

- Make every file, type, function and variable name [self-documenting]
  (https://en.wikipedia.org/wiki/Self-documenting_code).
- Try to name things for what value they provide to the user.
  Bad name: change_depth
  Good name: undo_history_index
- Avoid metaphors. A name that needs decoding is worse than a plain one.
- Rename rather than explain. A name needing a comment probably needs a better
  name.

### Split it up when you see

- A long file - it is holding several responsibilities.
- A long function - pull its steps into named helpers.
- Four or more levels of indentation - a helper is hiding in there.
- A long comment - the comment is too low-level, esoteric, and/or the name
  is not self-documenting enough.
- A file with many comments

### Comments

- Use comments sparingly and intentionally.
  - Types: Comment freely. Say what the type is responsible for. Length is fine.
  - Functions: Comment when necessary. i.e. wan argument or a behaviour needs
    clarification. Length is fine.
  - Bodies: Avoid comments inside function bodies when possible. Ideally, break
    the body into named helpers instead (easier said than done).
- Keep comments high level. Skip details the reader has no context for.
