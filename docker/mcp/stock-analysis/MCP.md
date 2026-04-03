# Stock Analysis MCP

Stock analysis on the Moscow Exchange (MOEX) via ISS API.

## Tools

### get_stock_price
- **ticker** (required): MOEX ticker (SBER, GAZP, LKOH)
Returns current price, daily change, range, volume.

### get_portfolio_summary
- **tickers** (required): Array of tickers
Returns prices for multiple tickers in a single request.

### search_ticker
- **query** (required): Search query (company name or partial ticker)
Searches MOEX securities by name.
