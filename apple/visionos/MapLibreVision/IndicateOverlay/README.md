# Indicate replay overlay

The bridge uses one self-contained Indicate source snapshot.
`vendor/indicate/SOURCE.json` records the base commit, working-tree inclusion, and every source file hash.
The snapshot includes matching state, scene, symbology, alerts, and HWD packages.
No build depends on another local checkout or an unpublished remote commit.

Run `python3 Scripts/sync-indicate.py /path/to/Indicate` from the app directory to update the snapshot.
Run `sh Scripts/check-flight-overlay.sh` to verify hashes and the bridge.
Run `sh Scripts/build-flight-overlay.sh aarch64-apple-visionos` to build the device library.
