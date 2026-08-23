// A `long int` survives being passed to a function and returned from one, and
// an `int` argument is widened to the parameter it is passed for.
//@ exit-code: 6

long int scale(long int value, long int by) {
    return value * by;
}

int main() {
    int factor = 3;
    long int scaled = scale(2000000000, factor);
    return scaled / 1000000000;
}
