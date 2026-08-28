// The shift count is computed rather than written down, so it has to reach the
// one register the hardware will take it from.
//@ exit-code: 96

int shift(int value, int by) {
    return value << by;
}

int main() {
    int base = 3;
    int amount = 2;
    return shift(base, amount * 2 + 1);
}
