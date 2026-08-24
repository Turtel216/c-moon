// A logical operator answers 1, not the operand that decided the answer.
//@ exit-code: 110

int main() {
    int a = 7;
    int b = 9;
    return (a && b) * 100 + (a || b) * 10 + !a;
}
