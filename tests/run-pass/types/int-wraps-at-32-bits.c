// `int` arithmetic is done 32 bits wide, so one past the largest `int` is the
// smallest one -- the wrap gcc performs at `-O0`. Widening the result
// afterwards shows it really is negative rather than merely truncated.
//@ exit-code: 1

int main() {
    int largest = 2147483647;
    int wrapped = largest + 1;
    long int widened = wrapped;
    return widened < 0;
}
