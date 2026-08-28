// De Morgan's laws, which hold bit for bit: the compiler must not confuse a
// complement with a logical not.
//@ exit-code: 7

int main() {
    int a = 12;
    int b = 10;

    if (~(a & b) != (~a | ~b)) {
        return 1;
    }
    if (~(a | b) != (~a & ~b)) {
        return 2;
    }
    if (~~a != a) {
        return 3;
    }
    // `!a` answers a question and `~a` flips bits; only one of them is 0 here.
    if (!a != 0) {
        return 4;
    }
    return (a ^ b) + 1;
}
