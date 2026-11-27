# Changelog

Each release's section becomes its GitHub release notes and appears on
https://wholeseek.utilitygods.com/changelog. Write for people using the app.

## 0.1.7

- A command line for scripts and AI agents (Linux and macOS): search, browse a user, download, check transfers, try another source, cancel. Nothing else: no chat, settings, shares, uploads, library or playback.
- It uses your saved login. If the app is open it works through the app; if not, a small background helper starts by itself and quits when idle. Only one Soulseek login is ever used.
- Open the app while the helper is downloading and the downloads move over to the app.
- Agent searches are rate limited so they stay polite to the network.
- The guide for agents is in docs/CLI.md.

## 0.1.6

- wholeseek for Windows: download the .zip, unzip it and run wholeseek.exe (Windows 10 or 11, 64-bit). Nothing else to install.
- First launch on Windows: if SmartScreen says "Windows protected your PC", choose More info → Run anyway.
- Show in folder opens Explorer with the file selected.
- Folder names that Windows doesn't allow are adjusted when you download them.
- Media keys aren't supported on Windows yet.

## 0.1.5

- Library: your own music in a tab. Albums, songs, instant search and stars (☆), with a Starred filter.
- Play a song, an album or the whole list; Next, Previous and media keys work as usual.
- Songs play right where they are: nothing is copied or downloaded, and the Library doesn't add anything to your shares.
- Choose the folders in Settings → Library folders (Music and Downloads to start). Scanning runs in the background and only re-reads files that changed.
- Users with thousands of folders open and scroll smoothly; long folder and file lists no longer lag.

## 0.1.4

- Right-click menus everywhere, and a ⋯ button wherever a menu exists: on tabs, sidebar folders, songs and search results.
- Open a folder or a user in a new tab from any menu, or with a middle-click on a folder.
- Tab menu: Duplicate tab, Close tab, Close other tabs, Close tabs to the right.
- Bigger, browser-like tabs; switching tabs no longer flashes.
- Media keys: Play/Pause, Next and Previous from the keyboard or headphones, even when wholeseek isn't in front. Shows up in the system's media controls.
- wholeseek tells you when a new version is out (a small notice next to Transfers). Your version and an on/off switch are in Settings.
- Opening a user's folder from search or the player now scrolls the folder list to it.
- With two tabs on the same user, both get the folder listing.
- Darker background.

## 0.1.3

- Search results grouped by album, with the number of copies; other copies are in the side panel.
- Sort results by relevance, availability, size, length or name.
- Wishlist (☆), recent searches, and "Showing results for…" when a search was widened.
- History in Transfers: search past downloads, show them in their folder, or download again.
- Cover art in the player, album headers and result rows.
- Results use the full width of large windows and fit beside the Transfers drawer.
- Downloading the same folder twice no longer hangs.

## 0.1.2

- Download to… any folder, with your recent folders one click away.
- Option to save single songs straight into the downloads folder.
- Tabs come back after a restart.
- Download history is remembered.
- New side panel for the selected album, and new icons.
- The playing song is highlighted in every tab; click its title to find it.
- Fixed text cut off at the left edge.

## 0.1.1

- Wide windows use the space on the right.

## 0.1.0

- First release: search Soulseek with format filters, browse users in tabs, press play to hear any track before you download it, play whole albums.
