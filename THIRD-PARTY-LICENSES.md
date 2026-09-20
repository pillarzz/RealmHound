# Third-Party Licenses

RealmHound's MIT license applies only to RealmHound-authored material.
Third-party components remain under the licenses listed below.

## Pixabay audio

Official RealmHound builds include notification audio under the
[Pixabay Content License](https://pixabay.com/service/license-summary/).
This audio is not licensed under RealmHound's MIT license.

## Npcap

RealmHound uses Npcap as an external Windows runtime dependency for packet
capture. The released executable links against Npcap SDK import libraries and
loads `wpcap.dll` from the user's Npcap installation at runtime.

RealmHound does not include or redistribute the Npcap installer, driver,
runtime DLLs, or SDK. Users must download and install Npcap separately from
the official website. RealmHound's CI and release workflows download the
Npcap SDK temporarily for linking; the SDK is not committed to this repository
or included in release assets.

Npcap is proprietary software governed by its own end-user license agreement,
not RealmHound's MIT license. Projects that want to bundle or redistribute
Npcap must obtain the applicable permission or OEM license from the Nmap
Project.

- Copyright: Copyright (c) 2013-2025 Nmap Software LLC
- Website and downloads: https://npcap.com/
- Npcap license: https://github.com/nmap/npcap/blob/master/LICENSE
- OEM and redistribution licensing: https://npcap.com/oem/
- Npcap SDK downloads: https://npcap.com/#download

## Noto fonts

RealmHound bundles these fonts under the SIL Open Font License, Version 1.1:

| File | Version | Copyright | SHA-256 |
|------|---------|-----------|---------|
| `realmhound/crates/realmhound/assets/NotoSans-Regular.ttf` | 2.015 | Copyright 2022 The Noto Project Authors | `fe8c022f48d8dd29f17b744d16f9346f4357e16f7d4f7be58b000ae7c291b614` |
| `realmhound/crates/realmhound/assets/NotoEmoji-Regular.ttf` | 3.005 | Copyright 2013 Google LLC | `575bc0aa43e620a0c440a07010c30f3b3b70148bd89679162040bfda08fe3856` |

Sources:

- Noto Sans: https://github.com/notofonts/latin-greek-cyrillic
- Noto Emoji: https://github.com/googlefonts/noto-emoji

```text
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to
provide a free and open framework in which fonts may be shared and
improved in partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software
components as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to,
deleting, or substituting -- in part or in whole -- any of the
components of the Original Version, by changing formats or by porting
the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed,
modify, redistribute, and sell modified and unmodified copies of the
Font Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the
corresponding Copyright Holder. This restriction only applies to the
primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

RealmHound includes portions of code ported from Java to Rust from the
following project. New features built on top of that foundation are original to
RealmHound.

## RealmShark

- Source: https://github.com/X-com/RealmShark
- License: MIT

RealmHound's protocol definitions, packet structures, and related parsing
logic were ported from RealmShark. Its loot-detection and attribution logic
were ported from RealmShark's `tomato` branch
(https://github.com/X-com/RealmShark/tree/tomato), and the `.wav` notification
sound files are also taken from that branch.

RealmHound's Quests, Combat History, Loot History, Party and Chat panels were
inspired by similar functions and features from RealmShark.

```
The MIT License (MIT)

Copyright (c) 2022

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Muledump

- Source: https://github.com/jakcodex/muledump
- License: BSD-3-Clause

RealmHound's Characters, Vault and Treasure panels were inspired by Muledump,
and parts of its dye and character-skin sprite rendering and RotMG web-API
request approach were inspired by Muledump's implementation.

```
BSD 3-Clause License

Copyright (c) 2017, Jakisaurus
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the copyright holder nor the names of its
  contributors may be used to endorse or promote products derived from
  this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
