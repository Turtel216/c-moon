// The test a logical operator makes is as wide as its operand's type: a
// `long int` whose low 32 bits are all zero is still true.
//@ exit-code: 2

int main() {
    long int high = 4294967296;
    return (high && 1) + (high || 0) + !high;
}
