// A `long int` holds values an `int` cannot: the sum below needs 33 bits, and
// dividing it back down proves none of them were lost.
//@ exit-code: 80

int main() {
    long int a = 4000000000;
    long int b = 4000000000;
    long int sum = a + b;
    return sum / 100000000;
}
