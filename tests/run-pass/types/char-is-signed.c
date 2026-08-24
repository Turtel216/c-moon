// Plain `char` is signed on the System V x86-64 ABI, so widening one to an
// `int` or a `long int` sign extends it rather than filling with zeroes.
//@ exit-code: 3

int main() {
    char negative = 255;
    int widened = negative;
    long int wider = negative;
    return (negative < 0) + (widened == 0 - 1) + (wider == 0 - 1);
}
