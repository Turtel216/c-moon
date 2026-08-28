// A `char` becomes an `int` before any operator sees it, so the complement of
// one is computed at 32 bits and not at 8.
//@ exit-code: 21

int main() {
    char a = 12;
    char b = 10;

    if ((~a) != -13) {
        return 1;
    }
    // The shift is an `int` shift, so the bit shifted past the eighth is not
    // lost the way a `char` would have lost it.
    if ((a << 4) != 192) {
        return 2;
    }
    return (a | b) + (a & b) - 1;
}
