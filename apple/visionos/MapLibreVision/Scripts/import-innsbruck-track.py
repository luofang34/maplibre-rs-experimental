#!/usr/bin/env python3
"""Create the bundled ADS-B approach from the provider recording and an EGM96 grid."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

from pyproj import Transformer

SOURCE = "https://globe.adsb.lol/globe_history/2026/09/08/traces/20/trace_full_440820.json"
GRID = "https://cdn.proj.org/us_nga_egm96_15.tif"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recording", type=Path, help=f"Downloaded gzip JSON: {SOURCE}")
    parser.add_argument("geoid", type=Path, help=f"Downloaded EGM96 grid: {GRID}")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    raw = args.recording.read_bytes()
    document = json.loads(gzip.decompress(raw))
    transform = Transformer.from_pipeline(
        "+proj=pipeline +step +proj=unitconvert +xy_in=deg +xy_out=rad "
        f"+step +inv +proj=vgridshift +grids={args.geoid.resolve()} +multiplier=1 "
        "+step +proj=unitconvert +xy_in=rad +xy_out=deg")
    # The next leg departs an hour later; it must never extend the arrival.
    points = [p for p in document["trace"] if 57335 <= p[0] <= 57740.78
              and p[9] == "adsb_icao" and p[10] is not None]
    if len(points) <= 30 or document["r"] != "OE-LWD" or document["timestamp"] != 1788825600:
        raise ValueError("The recording does not match the selected aircraft and date")
    if not any(p[8] and p[8].get("flight", "").strip() == "AUA10A" for p in points):
        raise ValueError("The approach callsign is missing from the recording")
    start = points[0][0]
    observations = []
    for p in points:
        gnss = p[10] * 0.3048
        _, _, msl = transform.transform(p[2], p[1], gnss)
        observations.append(dict(time=round(p[0] - start, 2), latitude=p[1], longitude=p[2],
            altitudeMSL=round(msl, 3), altitudeGNSS=round(gnss, 3),
            geoidSeparation=round(gnss - msl, 3), groundSpeed=round(p[4] * 1852 / 3600, 4),
            track=p[5], roll=p[13]))
    output = dict(title="Innsbruck approach", callsign="AUA10A", registration="OE-LWD", aircraft="Embraer E195",
        destination="Innsbruck · LOWI", startUTC=document["timestamp"] + start,
        source=dict(url=SOURCE, license="ODbL-1.0", sha256=hashlib.sha256(raw).hexdigest(),
            altitudeConversion="WGS84 GNSS feet → metres → EGM96 mean sea level; "
                f"{GRID}; grid SHA256 {hashlib.sha256(args.geoid.read_bytes()).hexdigest()}",
            coverage="Recorded approach, 8 September 2026. Receiver coverage ends before the runway. "
                "No landing or aircraft attitude is synthesized."), observations=observations)
    args.output.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n")
    print(f"Imported {len(points)} positions spanning {observations[-1]['time']:.2f} seconds")


if __name__ == "__main__":
    main()
