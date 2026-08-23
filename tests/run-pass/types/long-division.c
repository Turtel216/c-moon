// Division at 64 bits sign-extends into the whole of the dividend register
// pair, which is a different instruction from the one 32-bit division needs.
//@ exit-code: 90

int main() {
    long int big = 6000000000;
    long int quotient = big / 1000000000;
    int narrow = 105;
    int small = narrow / 7;
    return quotient * small;
}
