#!/usr/bin/env python3
"""Build the labeled valley-flight simulation; no ADS-B measurements are synthesized."""
import argparse
import hashlib
import io
import json
import math
from pathlib import Path
from urllib.request import urlopen

import numpy as np
from PIL import Image

# Geographic landmarks identify the corridor; they are not operational flight-plan fixes.
# Source: Mark Jayne's first-hand location guide, https://www.mjaviation.co.uk/MachLoop.htm
WAYPOINTS = [
    (52.723, -3.709), (52.731, -3.735), (52.740, -3.770),
    (52.751, -3.798), (52.750, -3.820), (52.731, -3.837),
    (52.709, -3.850), (52.695, -3.867), (52.682, -3.877),
    (52.672, -3.855), (52.654, -3.841), (52.626, -3.824),
    (52.609, -3.787), (52.623, -3.751), (52.641, -3.718),
    (52.668, -3.705), (52.695, -3.688), (52.717, -3.687),
    (52.729, -3.724), (52.740, -3.770), (52.751, -3.798),
]
RADIUS = 6371008.8
SCALE = np.array([math.pi * RADIUS / 180, math.pi * RADIUS * math.cos(math.radians(52.7)) / 180])


def path_samples():
    points = np.array(WAYPOINTS) * SCALE
    points = np.vstack([2 * points[0] - points[1], points, 2 * points[-1] - points[-2]])
    dense = []
    for i in range(1, len(points) - 2):
        a, b, c, d = points[i - 1:i + 3]
        for t in np.linspace(0, 1, 100, endpoint=False):
            dense.append(0.5 * (2*b + (-a+c)*t + (2*a-5*b+4*c-d)*t*t + (-a+3*b-3*c+d)*t*t*t))
    dense.append(points[-2])
    dense = np.array(dense)
    distances = np.r_[0, np.cumsum(np.linalg.norm(np.diff(dense, axis=0), axis=1))]
    arc = np.linspace(0, distances[-1], math.ceil(distances[-1] / 80) + 1)
    samples = np.column_stack([np.interp(arc, distances, dense[:, i]) for i in range(2)])
    return samples / SCALE, samples, arc


def terrain(coordinates, cache):
    tiles, hashes, elevations = {}, {}, []
    zoom, count = 13, 2**13
    for lat, lon in coordinates:
        x = (lon + 180) / 360 * count
        y = (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * count
        key = (int(x), int(y))
        if key not in tiles:
            url = f'https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{zoom}/{key[0]}/{key[1]}.png'
            file = cache / f'{zoom}-{key[0]}-{key[1]}.png'
            if not file.exists():
                with urlopen(url, timeout=30) as response:
                    file.write_bytes(response.read())
            raw = file.read_bytes()
            image = np.asarray(Image.open(io.BytesIO(raw)).convert('RGB'), dtype=float)
            tiles[key] = image[:, :, 0] * 256 + image[:, :, 1] + image[:, :, 2] / 256 - 32768
            hashes[url] = hashlib.sha256(raw).hexdigest()
        px, py = min(int((x % 1)*256), 255), min(int((y % 1)*256), 255)
        elevations.append(tiles[key][py, px])
    return np.array(elevations), hashes


def build(cache, output):
    coordinates, positions, arc = path_samples()
    ground, hashes = terrain(coordinates, cache)
    # Look-ahead terrain clearance prevents a synthetic camera from clipping a ridge.
    envelope = np.array([max(ground[max(0, i-12):i+13]) for i in range(len(ground))]) + 230
    altitude = np.convolve(np.pad(envelope, (8, 8), mode='edge'), np.ones(17)/17, mode='valid')
    altitude = np.maximum(altitude, ground + 200)
    north, east = np.gradient(positions[:, 0]), np.gradient(positions[:, 1])
    heading = np.unwrap(np.arctan2(east, north))
    curvature = np.gradient(heading, arc)
    speed = np.minimum(140, np.sqrt(9.80665 * math.tan(math.radians(40)) / np.maximum(abs(curvature), 1e-8)))
    elapsed = np.r_[0, np.cumsum(np.diff(arc) / ((speed[1:] + speed[:-1]) / 2))]
    pitch = np.degrees(np.arctan(np.gradient(altitude, arc)))
    roll = np.degrees(np.arctan(speed**2 * curvature / 9.80665))
    observations = []
    for i, (lat, lon) in enumerate(coordinates):
        observations.append(dict(time=round(float(elapsed[i]), 3), latitude=round(lat, 7), longitude=round(lon, 7),
            altitudeMSL=round(float(altitude[i]), 3), groundSpeed=round(float(speed[i]), 3),
            track=round(float(np.degrees(heading[i]) % 360), 5), heading=round(float(np.degrees(heading[i]) % 360), 5),
            roll=round(float(roll[i]), 4), pitch=round(float(pitch[i]), 4)))
    provenance = json.dumps(dict(waypoints=WAYPOINTS, terrain=hashes), sort_keys=True).encode()
    flight = dict(title='Conquering the Mach Loop', kind='simulation', callsign='MACH SIM', registration='Simulation',
        aircraft='Synthetic valley-flight camera', destination='Mach Loop, Wales', startUTC=0,
        source=dict(url='https://www.mjaviation.co.uk/MachLoop.htm', license='Original simulated track; terrain attribution: https://github.com/tilezen/joerd/blob/master/docs/attribution.md',
            sha256=hashlib.sha256(provenance).hexdigest(),
            altitudeConversion='Synthetic MSL altitude sampled from AWS Terrarium terrain plus at least 200 m clearance. No measured altitude or IAS.',
            coverage='A simulated valley circuit through Bwlch, Cad Pass, Corris and the Dyfi valley. This is not a recorded flight or an operational low-flying route. Speed, heading, pitch and bank are modeled; IAS is unavailable.'),
        observations=observations)
    output.write_text(json.dumps(flight, indent=2) + '\n')
    print(f'{len(observations)} samples, {elapsed[-1]:.1f} seconds, {arc[-1]/1000:.1f} km; clearance >= {(altitude-ground).min():.1f} m')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parents[1] / 'MapLibreVision/Resources/mach-loop.json')
    args = parser.parse_args()
    args.cache.mkdir(parents=True, exist_ok=True)
    build(args.cache, args.output)
