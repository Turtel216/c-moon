// The bitwise ladder: `|` binds loosest, then `^`, then `&`, all of them
// looser than a comparison, which is looser than a shift.
//@ exit-code: 35

int main() {
    // 1 | (2 ^ (3 & 4)) == 1 | (2 ^ 0) == 3
    if ((1 | 2 ^ 3 & 4) != 3) {
        return 1;
    }
    // 1 & (3 == 3) == 1 & 1 == 1
    if ((1 & 3 == 3) != 1) {
        return 2;
    }
    // 1 << (2 + 3) == 1 << 5 == 32
    if ((1 << 2 + 3) != 32) {
        return 3;
    }
    // A shift binds tighter than a comparison.
    if ((1 << 3 > 4) != 1) {
        return 4;
    }
    return (1 << 5) + (1 | 2);
}
