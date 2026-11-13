# Publish LuaBuddy's Workshop package

The [LuaBuddy Workshop item](https://steamcommunity.com/sharedfiles/filedetails/?id=3838682402) supplies a verified native Linux package and its graphical setup guide. Players extract the included archive and open `luabuddy` without terminal commands or another download. One-time activation remains separate from subscription. Native updates require opening the newly extracted package and applying the update, and unsubscribing alone does not uninstall the native integration.

The tracked [workshop folder](../workshop) owns the listing text, preview, and guide. `workshop.txt` is the description's source of truth. `preview.svg` is the editable preview source. Generate the required 256 by 256 PNG with `rsvg-convert workshop/preview.svg -o workshop/preview.png` when changing it. The existing item ID and [workshop-publisher.json](../workshop-publisher.json) keep updates attached to the same item. Do not create another item for routine updates.

The tracked folder does not contain generated native binaries. Upload the staged Workshop folder produced below. Uploading the raw tracked folder would omit the native setup package.

## Build and verify the native package

Build and package the release with the pinned toolchain and lockfile. Follow [verification](verification.md) before publishing new executable bytes. Record the exact runtime, CLI, manifest, and archive hashes and the source commit that built them. Keep proprietary game files, user configuration, logs, and third-party mods out of the package.

```sh
cargo build --release --locked --offline
target/release/luabuddy package --runtime target/release/libluabuddy_host.so --profiles-dir profiles --out dist/LuaBuddy-linux-x86_64
target/release/luabuddy verify-package --package dist/LuaBuddy-linux-x86_64
```

An offline build needs the locked dependencies already cached. The first graphical setup release uses tag `v0.1.0-rc.2`. Mark release candidates as prereleases. An optional GitHub release can carry the same verified native files, checksums, setup guide, and provenance as an alternative delivery source. Workshop players do not need that separate download.

The initial `v0.1.0-rc.1` release contained the native bytes accepted for PR #20 and a documentation-only Workshop item. Its provenance remains scoped to that artifact. A package containing the graphical executable needs its own verification and provenance.

## Stage the Workshop download

GNU tar is a publishing dependency. It is not required by the graphical installer. The output's parent directory must exist, and the output must be outside both the source guide and native package directories.

```sh
cargo xtask workshop --package dist/LuaBuddy-linux-x86_64 --out dist/LuaBuddy-Workshop
```

The staging command verifies the supplied package and copies the tracked guide, preview, and metadata. It creates `Contents/mods/LuaBuddy_SetupGuide/common/setup/LuaBuddy-Setup-linux-x86_64.tar.gz` and its checksum. The archive contains the existing package layout inside a `LuaBuddy-<package-digest-prefix>` folder. Its Unix file modes allow a desktop archive manager to restore the executable permission on `luabuddy`. The installer still verifies the extracted manifest and files before mutation.

The archive is a distribution container, not an automatically executed installer. No Lua shim, extra interpreter, alternate loader, or upstream mod is added. Graphical mode uses an existing Zenity or KDialog desktop utility. Keep the listing and guide explicit about that Linux desktop prerequisite and the required user activation.

Update release links and filenames in `workshop.txt` and the packaged setup guide when the public package changes. Check graphical setup, maintenance, removal, platform scope, and current limitations before uploading.

## Upload the staged content

Use the shared [Steam Workshop publisher](https://github.com/DevGuyRash/steam-workshop-publisher) from its checkout. Its [agent quickstart](https://github.com/DevGuyRash/steam-workshop-publisher/blob/main/docs/agent-quickstart.md) and [desktop guide](https://github.com/DevGuyRash/steam-workshop-publisher/blob/main/docs/desktop.md) own the generic procedure. A local checkout is named `SteamWorkshopPublisher` under the same parent directory as this repository.

```sh
python3 folder.py ../LuaBuddy/dist/LuaBuddy-Workshop --dry-run
python3 desktop.py probe --app-id 108600
python3 folder.py ../LuaBuddy/dist/LuaBuddy-Workshop --publish --backend desktop --notes-file /path/to/change-notes.txt
```

Pin `--backend desktop` to reuse the current signed-in Steam session. Do not select a separate SteamCMD login for this local workflow. The shared publisher validates and uploads `Contents` and change notes. It preserves the existing title, description, preview, tags, visibility, and required items. Changes to `workshop.txt` alone therefore do not edit the live page. Use the game's supported uploader with the staged folder for intentional listing metadata changes.

The initial item was created privately with the game's own Workshop API, received guide content through the shared publisher, and used the game's uploader to publish its title, description, preview, and public visibility. It has no required Workshop items. All bundled runtime integrations remain optional and their upstream mods are not included.

## Check the delivered player flow

Confirm the exact item ID and Steam success result, then read the public page while signed out. Check the formatted description, preview, setup path, optional release link, change notes, and visibility. Verify the actual Steam-delivered archive, graphical extraction, file-manager launch from another working directory, installation result, and subsequent native launch in a disposable game environment. Exercise updates and maintenance without changing the user's running game or profiles. Keep evidence outside Git and report the exact scope that ran.

If a request times out, inspect the existing item before retrying. A passing content upload proves delivery only. It does not prove native activation, every desktop environment, or gameplay support.
