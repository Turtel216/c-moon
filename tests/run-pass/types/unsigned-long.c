// An `unsigned long int` holds values no signed type can, including the
// literals too large for a `long int` that name them.
//@ exit-code: 7

int main() {
    unsigned long int all_ones = 18446744073709551615;
    unsigned int u = 4294967295;
    unsigned long int widened = u;

    return (all_ones == 18446744073709551615)
         + (widened == 4294967295) * 2
         + (all_ones / 3 == 6148914691236517205) * 4;
}
