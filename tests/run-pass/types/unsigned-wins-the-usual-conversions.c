// An `int` and an `unsigned int` meet as `unsigned int`, because no type here
// can represent every value of both. Against a `long int` the wider signed
// type is enough, so it wins instead.
//@ exit-code: 3

int main() {
    unsigned int one = 1;
    int negative = 0 - 1;
    // The -1 becomes the largest `unsigned int` there is, so this is true.
    int surprising = (one < negative);

    long int wide_negative = 0 - 1;
    // Here the comparison stays signed, so this one is true as well.
    int expected = (one > wide_negative);

    return surprising + expected * 2;
}
