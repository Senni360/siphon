"""Conservative empirical confidence mapping.

Scores express exact BPM (within one beat/minute) and exact major/minor key,
never octave-equivalent tempo or harmonically related key correctness.
"""


def calibrate(kind, quality):
    quality = max(0., min(1., float(quality)))
    # PAVA + one-sided 90% Wilson bounds, from 54 development recordings
    # (47 explicit keys). Taper the
    # unsupported near-zero region to zero for silence/no periodicity/tonality.
    if kind == "bpm":
        value = .576 if quality < .6 else .931
        value *= min(1., quality / .2)
    elif kind == "key":
        value = .36 if quality < .2 else .682 if quality < .8 else .81
        value *= min(1., quality / .1)
    else:
        raise ValueError("Unknown analysis confidence kind")
    return round(value, 3)
