// The usual arithmetic conversions: an `int` operand meeting a `long int` one
// is widened, and the addition happens at 64 bits.
//@ exit-code: 3

int main() {
    int small = 2;
    long int big = 2999999998;
    long int total = big + small;
    return total / 1000000000;
}
