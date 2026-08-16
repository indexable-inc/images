#
# Copyright © 2016 Intel Corporation
#
# SPDX-License-Identifier: MIT

import argparse
import sys

# fuse fadd+fmul late to get something we can turn into mad.f32/f16.  The
# common nir_opt_algebraic_late pass only does this for non-exact patterns.
# Since for us, mad is not fused, we don't have this restriction.
late_optimizations = []

a = 'a'
b = 'b'
c = 'c'

for sz in [16, 32]:
    # Fuse the correct fmul. Only consider fmuls where the only users are fadd
    # (or fneg/fabs which are assumed to be propagated away), as a heuristic to
    # avoid fusing in cases where it's harmful.
    fmul = 'fmul(is_only_used_by_fadd)'
    ffma = 'ffma'

    fadd = 'fadd@{}'.format(sz)

    late_optimizations.extend([
        ((fadd, (fmul, a, b), c), (ffma, a, b, c)),

        ((fadd, ('fneg(is_only_used_by_fadd)', (fmul, a, b)), c),
         (ffma, ('fneg', a), b, c)),

        ((fadd, ('fabs(is_only_used_by_fadd)', (fmul, a, b)), c),
         (ffma, ('fabs', a), ('fabs', b), c)),

        ((fadd, ('fneg(is_only_used_by_fadd)', ('fabs', (fmul, a, b))), c),
         (ffma, ('fneg', ('fabs', a)), ('fabs', b), c)),
    ])

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('-p', '--import-path', required=True)
    args = parser.parse_args()
    sys.path.insert(0, args.import_path)
    run()


def run():
    import nir_algebraic  # pylint: disable=import-error

    print('#include "ir3_nir.h"')
    print(nir_algebraic.AlgebraicPass("ir3_nir_opt_algebraic_late",
                                      late_optimizations).render())


if __name__ == '__main__':
    main()
