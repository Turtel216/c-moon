// An unsigned value has no sign to keep, so its right shift brings zeroes in
// at the top: the same bits the signed shift leaves alone.
//@ exit-code: 1

unsigned int all_ones() {
    unsigned int value = 0;
    return ~value;
}

int main() {
    unsigned int wide = all_ones() >> 1;

    if (wide != 2147483647) {
        return 2;
    }
    // The same bit pattern read as an `int` keeps its sign instead.
    int narrow = -1;
    if ((narrow >> 1) != -1) {
        return 3;
    }
    return 1;
}
