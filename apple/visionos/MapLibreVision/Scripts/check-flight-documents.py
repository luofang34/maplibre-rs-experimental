#!/usr/bin/env python3
"""Check the built bundle's file-sharing contract with Launch Services."""

import plistlib
import sys
from pathlib import Path


def read(path):
    with path.open("rb") as source:
        return plistlib.load(source)


app = Path(sys.argv[1])
info = read(app / "Info.plist")
handlers = info["CFBundleDocumentTypes"]
accepted = {uti for handler in handlers for uti in handler["LSItemContentTypes"]}
required = {"org.topografix.gpx", "com.google.earth.kml", "com.sokolysystems.flight-track"}
assert required <= accepted, "Missing flight document handler"
assert all(handler["CFBundleTypeRole"] == "Viewer" for handler in handlers)
assert info["LSSupportsOpeningDocumentsInPlace"], "External files need scoped URL access"
declarations = {
    item["UTTypeIdentifier"]: item
    for key in ("UTImportedTypeDeclarations", "UTExportedTypeDeclarations")
    for item in info.get(key, [])
}
for uti in required:
    declaration = declarations[uti]
    assert {"public.data", "public.content"} <= set(declaration["UTTypeConformsTo"]), uti
    assert declaration["UTTypeTagSpecification"]["public.filename-extension"], uti

extension = read(app / "PlugIns/TrackShare.appex/Info.plist")
configuration = extension["NSExtension"]
assert configuration["NSExtensionPointIdentifier"] == "com.apple.share-services"
assert configuration["NSExtensionPrincipalClass"] == "TrackShare.ShareTrackViewController"
rule = configuration["NSExtensionAttributes"]["NSExtensionActivationRule"]
assert rule["NSExtensionActivationSupportsFileWithMaxCount"] == 1
assert extension["CFBundleIdentifier"].startswith(info["CFBundleIdentifier"] + ".")
print("Built flight document handlers and Share extension registration passed")
