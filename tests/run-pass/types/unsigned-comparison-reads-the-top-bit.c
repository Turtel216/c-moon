// The top bit of an unsigned value is a digit rather than a sign, so the same
// bits that make a negative `int` make the largest `unsigned int` there is.
//@ exit-code: 7

int main() {
    unsigned int big = 4294967295;
    unsigned int small = 1;
    int negative = 0 - 1;

    return (big > small) + (negative < 0) * 2 + ((unsigned int) negative == big) * 4;
}
