// A loop whose bound has its top bit set runs only if the comparison reads
// that bit as a digit: as an `int` the limit would be negative and the body
// would never be entered.
//@ exit-code: 3

int main() {
    unsigned int limit = 2147483650;
    unsigned int i = 2147483647;
    unsigned int steps = 0;

    while (i < limit) {
        i = i + 1;
        steps = steps + 1;
    }

    return steps;
}
