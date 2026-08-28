// A 64-bit shift reads a sixth bit of its count, so it can move a value
// further than any `int` shift could.
//@ exit-code: 33

long int main() {
    long int one = 1;
    long int high = one << 40;

    if (high != 1099511627776) {
        return 1;
    }
    if ((high >> 40) != 1) {
        return 2;
    }
    // The masks are as wide as the operands they meet.
    long int mask = high | 32;
    return (int) (mask & 63) + 1;
}
