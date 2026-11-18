"""Logo variants stay generated from the background-free source."""

import math
import re

from _tools.render_logo import (ASSETS, BACKGROUND, CENTER, OPTICAL_SHIFT, SAFE_RADIUS, SIZE, SOURCE, VARIANTS,
                                main, outline_points, render)


def test_committed_variants_match_the_source():
    assert main(["--check"]) == 0


def test_source_has_no_background_and_each_variant_adds_one():
    source = SOURCE.read_text(encoding="utf-8")
    assert BACKGROUND not in source.split("<title>Gutch</title>", 1)[1].split("<path", 2)[1]
    for name, background in VARIANTS.items():
        variant = (ASSETS / name).read_text(encoding="utf-8")
        assert variant == render(source, background)
        assert variant.index(background) < variant.index("<path")


def test_variants_place_the_drawing_at_the_optical_center_inside_the_safe_circle():
    source = SOURCE.read_text(encoding="utf-8")
    for name in VARIANTS:
        variant = (ASSETS / name).read_text(encoding="utf-8")
        to_x, to_y, scale, from_x, from_y = map(float, re.search(
            r"translate\((\S+) (\S+)\) scale\((\S+)\) translate\((\S+) (\S+)\)", variant).groups())
        placed = [(to_x + scale * (x + from_x), to_y + scale * (y + from_y)) for x, y in outline_points(source)]
        xs, ys = [x for x, _ in placed], [y for _, y in placed]
        assert abs((min(xs) + max(xs)) / 2 - (CENTER[0] + OPTICAL_SHIFT * SIZE)) < 1
        assert abs((min(ys) + max(ys)) / 2 - CENTER[1]) < 1
        assert max(math.dist(point, CENTER) for point in placed) <= SAFE_RADIUS * SIZE / 2 + 1


def test_committed_banner_matches_its_sources():
    from _tools.render_banner import main as render_banner

    assert render_banner(["--check"]) == 0
