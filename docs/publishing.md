# Publish LuaBuddy's Workshop page

The [LuaBuddy Workshop item](https://steamcommunity.com/sharedfiles/filedetails/?id=3838682402) contains a setup guide. The native Linux package is distributed through [GitHub releases](https://github.com/DevGuyRash/LuaBuddy/releases). The game's uploader rejects native `.so` files, and subscribing to a guide does not activate a JVM agent. Keep that distinction visible in the listing and packaged documentation.

The tracked [workshop folder](../workshop) owns the listing text, preview, and documentation payload. `workshop.txt` is the description's source of truth. `preview.svg` is the editable preview source. Generate the required 256 by 256 PNG with `rsvg-convert workshop/preview.svg -o workshop/preview.png` when changing it. The metadata uses the existing item ID. [workshop-publisher.json](../workshop-publisher.json) records the mapping for the shared publisher. Do not create another item for routine updates.

## Prepare a native release

Use a verified native package. Record its exact runtime, CLI, manifest, and archive hashes and the source commit that built them. Follow [verification](verification.md) before publishing new executable bytes. Reusing an already verified package for a documentation-only publication requires a fresh checksum and `verify-package` check, not a claim that new source was compiled.

Publish a GitHub release with the native archive, `SHA256SUMS`, and build provenance. Mark release candidates as prereleases. The initial `v0.1.0-rc.1` package contains version `0.1.0` and the native bytes accepted for PR #20. Its provenance names the actual source commit. Workshop documentation commits do not change those bytes.

Update the release links and filenames in both `workshop.txt` and the packaged setup guide when the public package changes. Check the release download, installation instructions, repair/removal links, platform scope, and current limitations before publishing the listing.

## Upload guide content

Use the shared [Steam Workshop publisher](https://github.com/DevGuyRash/steam-workshop-publisher) from its checkout. Its [agent quickstart](https://github.com/DevGuyRash/steam-workshop-publisher/blob/main/docs/agent-quickstart.md) and [desktop guide](https://github.com/DevGuyRash/steam-workshop-publisher/blob/main/docs/desktop.md) own the generic procedure. A local checkout is named `SteamWorkshopPublisher` under the same parent directory as this repository.

```sh
python3 folder.py ../LuaBuddy/workshop --dry-run
python3 desktop.py probe --app-id 108600
python3 folder.py ../LuaBuddy/workshop --publish --backend desktop --notes-file /path/to/change-notes.txt
```

Pin `--backend desktop` to reuse the current signed-in Steam session. Do not select a separate SteamCMD login for this local workflow. The shared publisher validates and uploads `Contents` and change notes. It preserves the existing title, description, preview, tags, visibility, and required items. Changes to `workshop.txt` alone therefore do not edit the live page. Use the game's supported uploader for intentional listing metadata changes.

The initial item was created privately with the game's own Workshop API, received guide content through the shared publisher, and used the game's uploader to publish its title, description, preview, and public visibility. It has no required Workshop items. All bundled runtime integrations remain optional and their upstream mods are not included.

After an upload, confirm the exact item ID and Steam success result, then read the public page while signed out. Check the rendered description, preview, release link, change notes, and public visibility. Keep local upload evidence outside Git. If a request times out, inspect the existing item before retrying. A successful content upload does not establish native activation or gameplay support.
