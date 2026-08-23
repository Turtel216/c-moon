// `int` and `long int` are both signed, so widening one to the other sign
// extends: a negative `int` stays negative rather than becoming a large
// positive number.
//@ exit-code: 1

int main() {
    int negative = 0 - 5;
    long int widened = negative;
    return widened < 0;
}
