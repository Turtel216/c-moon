// A lone `;` is a statement that does nothing. It is legal on its own, as the
// body of a loop that does all its work in its header, and as a branch of an
// `if`.
//@ exit-code: 6

int main() {
    int sum = 0;
    ;

    // The header counts on its own, so the body is empty.
    for (int i = 0; i < 3; i = i + 1)
        ;

    if (sum > 0)
        ;
    else
        sum = 6;

    while (0)
        ;

    return sum;
}
