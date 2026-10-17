# Third party notices

This app bundles and ports work from other free software projects. The licences
below require their notices to travel with the code and the data, so they are
collected here.

## Patch interpreter

The interpreter in `src/engine/` is a Rust port of the interpreter from:

    Apollo Save Tool library
    Copyright (C) 2020-2026 Damian Parrino
    https://github.com/bucanero/apollo-lib

    This program is free software: you can redistribute it and/or modify it
    under the terms of the GNU General Public License as published by the Free
    Software Foundation, either version 3 of the License, or (at your option)
    any later version.

The port keeps the same behaviour for address modes, checksums and ciphers.

## Bundled code database

`assets/codes.json` merges two community PS3 code sets. The larger of the two
is the Apollo Save Tool patch database:

    Apollo Save Tool Online Database
    Copyright (C) 2020-2026 Damian Parrino
    https://github.com/bucanero/apollo-patches

    This program is free software: you can redistribute it and/or modify it
    under the terms of the GNU General Public License as published by the Free
    Software Foundation, either version 3 of the License, or (at your option)
    any later version.

Each cheat also carries the original author credit that its patch file lists.

## Fallback title list

`assets/titles.tsv` is derived from the community GameDB-PS3 title database:

    https://github.com/niemasd/GameDB-PS3

See that project for its licence.

## This project

Because the ported code and the bundled data are both GPL-3.0, this project as a
whole is released under the GNU General Public License, version 3 or later. The
full text is in [LICENSE](LICENSE).
