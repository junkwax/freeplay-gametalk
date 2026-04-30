# Notice

freeplay-gametalk is the client package for the Freeplay app.

This repository does not include ROM files, OAuth secrets, webhooks, private
service credentials, or compiled third-party emulator cores.

## FBNeo

The optional FBNeo libretro core is built from the upstream FBNeo source tree:

https://github.com/finalburnneo/FBNeo

FBNeo is distributed under a non-commercial license. Do not sell packages that
include FBNeo, and do not use FBNeo as part of a commercial product or activity.
Build helper scripts in `tools/` clone FBNeo locally into `vendor/FBNeo` and
copy compiled cores into `cores/`; both folders are ignored by git.

## ROMs

Users must provide their own legally obtained compatible ROM zip. ROM files are
not distributed with this project.

## Fonts

`src/media/mk2.ttf` is tracked as the app font. Its upstream source is:

https://www.mortalkombatwarehouse.com/site/fonts/mortalkombat2.ttf

Other local test fonts are ignored by git.
