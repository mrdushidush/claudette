package shop;

/** Computes sales tax for a given amount. */
public class TaxCalculator {
    private final double rate;

    public TaxCalculator(double rate) {
        this.rate = rate;
    }

    /** Returns the tax owed on a pre-tax amount. */
    public double computeTax(double amount) {
        return amount * rate;
    }
}
