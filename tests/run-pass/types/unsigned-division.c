// An unsigned division is a different instruction from a signed one, not just
// a different reading: halving the largest `unsigned int` gives almost as
// large a number, where halving the same bits as an `int` gives zero.
//@ exit-code: 7

unsigned int half(unsigned int value) {
    return value / 2;
}

int main() {
    unsigned int halved = half(4294967295);
    int signed_halved = (0 - 1) / 2;

    return (halved == 2147483647) + (signed_halved == 0) * 2 + (half(10) == 5) * 4;
}
