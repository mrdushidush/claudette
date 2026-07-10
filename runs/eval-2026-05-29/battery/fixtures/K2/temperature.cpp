#include "temperature.h"

// Convert Celsius to Fahrenheit. Formula: F = C * 9 / 5 + 32.
double celsiusToFahrenheit(double c) {
    return c * 9.0 / 5.0 - 32.0;   // returns the wrong value
}
