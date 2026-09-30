-- Synthetic rows for the local-ops tests (reference, connections). Invented ids and names.
-- A deleted airport and airline: lookups must skip them.
INSERT INTO Airport (id,name,iata,icao,city,country,countryCode,timeZoneIdentifier,latitude,longitude,relevance,created,lastUpdated,deleted) VALUES
 ('a0000000-0000-4000-8000-000000000030','Closed Test Field','ZZD','ZZZD','Nowhere','Testland','XT','UTC',0,0,1,0,0,1);
INSERT INTO Airline (id,name,iata,icao,relevance,created,lastUpdated,deleted) VALUES
 ('b0000000-0000-4000-8000-000000000030','Deleted Test Air','ZZ','ZZT',1,0,0,1);
-- A deleted connection (skipped) and a past one from a Flight into a ManualFlight whose
-- departure is before the inbound arrival (risk "missed", no known MCT).
INSERT INTO Connection (accountId,id,userId,arrivingFlightId,departingFlightId,waitingAirportId,mctMinutes,created,lastUpdated,deleted) VALUES
 (0,'d0000000-0000-4000-8000-000000000030','00000000-0000-4000-8000-000000000001','f0000000-0000-4000-8000-000000000004','f0000000-0000-4000-8000-000000000009','a0000000-0000-4000-8000-000000000005',30,0,0,1),
 (0,'d0000000-0000-4000-8000-000000000031','00000000-0000-4000-8000-000000000001','f0000000-0000-4000-8000-000000000005','e0000000-0000-4000-8000-000000000001','a0000000-0000-4000-8000-000000000001',NULL,0,0,NULL);
-- Steps for connection d...01, as the app stores them (protobuf: field 8 = step, 1 = title, 2 = text).
INSERT INTO ConnectionSteps (accountId,id,connectionId,arrivingFlightId,departingFlightId,userId,state,stepsData,created,lastUpdated) VALUES
 (0,'d0000000-0000-4000-8000-000000000032','d0000000-0000-4000-8000-000000000001','f0000000-0000-4000-8000-000000000001','f0000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001','steps',
  X'421A0A09446973656D6261726B120D466F6C6C6F77207369676E732E420A0A085365637572697479',0,0);
